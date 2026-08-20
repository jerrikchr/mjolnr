//! Session lifecycle against the durable store.
//!
//! One reason to change: how a session is created, resumed, checkpointed, and
//! shut down.
//!
//! Everything here runs inside the actor task, so it is the only writer and
//! needs no lock (`AGENTS.md` §2.3).

use std::sync::Arc;

use crate::core::checkpoint::SessionCheckpoint;
use crate::core::continuation::{QuotaReservePhase, ResumeAdvice, ResumeWarning};
use crate::core::error::{MjolnrError, ReasonCode};
use crate::core::event::{MjolnrEvent, SessionId, StoredEvent};
use crate::core::policy::PolicyMode;
use crate::core::recovery::{RecoveryDecision, RecoveryState};
use crate::core::store::{SessionStatus, SessionSummary, StoreError};
use crate::runtime::recovery;
use crate::runtime::session::SessionState;

use super::Actor;

/// The outcome of attempting to register a discovered extension.
#[derive(Debug)]
enum Registration {
    /// Freshly loaded; carries the program the extension runs.
    Loaded(String),
    /// A tool of that name — a built-in or an already-loaded extension — is
    /// already callable, so nothing was registered or recorded.
    AlreadyAvailable,
    /// No discovered extension has that name.
    NotFound,
}

/// The title a new session gets.
///
/// Phase 4 does not ask the model to name sessions; the directory is what a
/// human actually recognises in a list.
fn default_title(state: &SessionState) -> String {
    state
        .workspace_root
        .as_ref()
        .and_then(|root| root.file_name())
        .map_or_else(
            || "session".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
}

impl Actor {
    /// Open a project root, canonicalised off the async worker.
    ///
    /// Every exit is either the root being set or a typed refusal the caller
    /// receives. The earlier version returned `()`: a run or open session made
    /// it return silently, and an unopenable path was reported as
    /// `StoreError::Unavailable`, which told the user their database was broken
    /// when they had mistyped a directory. A refusal nobody is told about is
    /// indistinguishable from a dead button (AGENTS.md §1.3).
    pub(super) async fn open_project(
        &mut self,
        root: std::path::PathBuf,
    ) -> Result<(), MjolnrError> {
        if self.run.is_some() {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::RunActive,
                "the workspace root cannot change while a run is active; cancel the run first",
            ));
        }
        if self.state.session.is_some() {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceRootLocked,
                "a session is already open on this workspace root; end the session before \
                 opening a different one",
            ));
        }
        let canonical =
            tokio::task::spawn_blocking(move || crate::policy::paths::canonical_root(&root)).await;
        match canonical {
            Ok(Ok(root)) => {
                self.load_rules_snapshot(root.clone()).await;
                self.state.workspace_root = Some(root);
                self.refresh_session_list().await;
                self.refresh_memory_projection().await;
                // The first of the four D5 refresh triggers. Opening a project
                // is the only one that can change which repository is being
                // described, so a stale projection here would attribute one
                // repository's branch and dirty count to another.
                self.refresh_repository(crate::core::repository::RefreshTrigger::ProjectOpened)
                    .await;
                Ok(())
            }
            // `canonical_root` already graded this refusal; carrying its own
            // code through keeps one authority over what an unopenable root
            // means rather than re-labelling it here.
            Ok(Err(refusal)) => Err(MjolnrError::workspace_refused(refusal.code, refusal.detail)),
            // A join failure leaves nothing written — `canonical_root` is a
            // pure read — so this is a failed attempt, not a store fault and
            // not an uncertain effect.
            Err(error) => Err(MjolnrError::workspace_refused(
                ReasonCode::ToolExecution,
                format!("workspace validation task did not complete: {error}"),
            )),
        }
    }

    pub(super) async fn refresh_session_list(&mut self) {
        match self.store.sessions().await {
            Ok(sessions) => {
                self.state.sessions = Arc::new(sessions);
                self.publish_snapshot();
            }
            Err(error) => self.note_store_failure(&error),
        }
    }

    /// Rewind the active leaf so the next message branches from `sequence`'s
    /// parent.
    ///
    /// The session's in-memory transcript is rebuilt from the branch that now
    /// ends at the new leaf, so what the provider sees next matches what the
    /// store says the branch is. Tool calls on the abandoned branch are not
    /// re-executed: they are recorded history, and a rewind reads history, it
    /// does not replay it.
    pub(super) async fn rewind_to(&mut self, sequence: u64) {
        let Some(session) = self.state.session else {
            return;
        };
        if self.run.is_some() {
            return;
        }
        // The branch about to be left, summarised while it is still the branch:
        // read after the leaf moves and this would describe where we landed
        // rather than what we walked away from.
        let left = self.summarise_current_branch(session).await;

        // Rewinding to the parent of `sequence` means the event at `sequence`
        // is itself dropped from the branch — that is what "go back to before
        // this message" means to the person selecting it.
        let leaf = sequence.checked_sub(1);
        if let Err(error) = self.store.set_active_leaf(session, leaf).await {
            self.note_store_failure(&error);
            return;
        }
        let events = match self.store.branch_events(session).await {
            Ok(events) => events,
            Err(error) => {
                self.note_store_failure(&error);
                return;
            }
        };
        if let Err(error) = self.state.rebuild_messages_from(&events) {
            self.note_store_failure(&error);
            return;
        }
        self.state.left_branch = left;
        // The next durable event branches from here rather than continuing the
        // line it was just pulled off.
        self.state.branch_parent = Some(leaf);
        // Same staleness as `follow_branch`: the leaf moved, so which nodes are
        // on the active branch changed under whoever is looking at the tree.
        if let Ok(tree) = self.store.session_tree(session).await {
            self.state.tree = Arc::new(tree);
        }
        self.publish_snapshot();
    }

    /// Rollback session transcript and state to a verified checkpoint.
    ///
    /// Persists intent before effect, checks git head consistency if supplied,
    /// and resets the session leaf and messages to the checkpoint boundary.
    pub(super) async fn rollback_to_checkpoint(
        &mut self,
        target_sequence: u64,
        expected_head: Option<String>,
    ) {
        if self.state.session.is_none() || self.run.is_some() {
            return;
        }

        if let (Some(expected), crate::core::repository::RepositoryView::Projected(proj)) =
            (expected_head.as_deref(), &self.state.repository)
            && proj.head.as_deref().is_some_and(|head| head != expected)
        {
            return;
        }

        self.rewind_to(target_sequence).await;
    }

    /// Start a new session carrying this branch forward.
    ///
    /// `before` cuts the history: `Some(sequence)` forks at the same point a
    /// rewind would, leaving the original session untouched; `None` clones the
    /// whole active branch.
    ///
    /// The carried messages are re-appended as this session's own events rather
    /// than referenced. A session's history is its history — pointing at
    /// another session's events would make the fork unreadable the moment the
    /// original was pruned, and would make "what did this session see?"
    /// a question with two answers.
    ///
    /// What crosses and what does not is documented on
    /// [`MjolnrCommand::ForkSession`](crate::core::command::MjolnrCommand::ForkSession).
    /// The short version: inert context crosses, authority does not.
    pub(super) async fn fork_session(&mut self, before: Option<u64>) {
        let (Some(_), Some(provider), Some(model)) = (
            self.state.session,
            self.state.provider.clone(),
            self.state.model.clone(),
        ) else {
            return;
        };
        if self.run.is_some() || self.blocked().is_some() {
            return;
        }
        let Some(root) = self.state.workspace_root.clone() else {
            return;
        };

        // Everything that crosses, read before the reset wipes it.
        let carried: Vec<crate::core::message::CanonicalMessage> = self
            .state
            .messages()
            .iter()
            .filter(|entry| match (before, entry.sequence) {
                // The cut is exclusive, matching a rewind: forking "at" a turn
                // means the new session starts just before it was said.
                (Some(cut), Some(sequence)) => sequence < cut,
                // No cut is a clone, and an unanchored entry has no position to
                // compare against one — it is a compaction seed, so it precedes
                // everything anchored and dropping it would lose the context it
                // carries. Both cross.
                (None, _) | (Some(_), None) => true,
            })
            .map(|entry| entry.message.clone())
            .collect();
        let policy = self.state.policy;
        let read_set = match self.state.read_set.entries() {
            Ok(entries) => entries,
            Err(error) => {
                self.note_store_failure(&StoreError::Unavailable {
                    detail: error.to_string(),
                });
                return;
            }
        };
        let activated_skills = self.state.activated_skills.clone();
        let workspace_trusted = self.state.workspace_trusted;
        let handoff = self.state.handoff.clone();

        if let Err(error) = self.release_lease().await {
            self.note_store_failure(&error);
            return;
        }

        self.state.reset_keeping_project();
        self.state.workspace_root = Some(root);
        // Set before `create_session`, which reads it and clamps it. Exact
        // command grants are deliberately not restored: `reset_keeping_project`
        // dropped them and nothing here puts them back.
        self.state.policy = policy;
        self.create_session(provider, model).await;
        let Some(session) = self.state.session else {
            return;
        };

        for message in carried {
            if let Err(error) = self
                .persist(MjolnrEvent::MessageAppended {
                    session,
                    message: Box::new(message),
                })
                .await
            {
                self.note_store_failure(&error);
                return;
            }
        }
        // The transcript is rebuilt from what was just written rather than
        // pushed as it goes, so the entries carry *this* session's sequences.
        // Reusing the numbers they had in the session they came from would
        // point `/tree` at events in a different session.
        match self.store.branch_events(session).await {
            Ok(events) => {
                if let Err(error) = self.state.rebuild_messages_from(&events) {
                    self.note_store_failure(&error);
                    return;
                }
            }
            Err(error) => {
                self.note_store_failure(&error);
                return;
            }
        }

        // Not defaulted on failure. An empty read set is not a safe fallback:
        // it is what lets the forked session edit files whose versions it never
        // verified, so a set that cannot be carried is a failure, not a blank.
        match crate::core::tool::ReadSet::restore(read_set) {
            Ok(set) => self.state.read_set = Arc::new(set),
            Err(error) => {
                self.note_store_failure(&StoreError::Unavailable {
                    detail: error.to_string(),
                });
                return;
            }
        }
        self.state.activated_skills = activated_skills;
        self.state.workspace_trusted = workspace_trusted;
        self.state.handoff = handoff;
        self.publish_snapshot();
    }

    /// Summarise the branch currently being followed, for the moment before
    /// it stops being followed.
    ///
    /// A deterministic projection of recorded events; no model is called. An
    /// empty summary — a branch on which nothing happened — is reported as
    /// `None`, because "you left a branch where nothing happened" is noise
    /// rather than news.
    ///
    /// A store that cannot answer produces `None` rather than a store failure:
    /// this is a courtesy read, and failing a rewind because its footnote could
    /// not be assembled would trade the operation for the note about it.
    async fn summarise_current_branch(
        &self,
        session: SessionId,
    ) -> Option<crate::core::store::BranchSummary> {
        let leaf = self
            .state
            .messages()
            .iter()
            .rev()
            .find_map(|entry| entry.sequence)?;
        let summary = self.store.branch_summary(session, leaf).await.ok()?;
        (!summary.is_empty()).then_some(summary)
    }

    /// Read the session tree onto the snapshot.
    ///
    /// A read, never a write. It is also the only projection that looks at
    /// branches the session is not on: everything else here deliberately reads
    /// the active branch, and a tree that showed only the branch you are
    /// standing on would be a list.
    pub(super) async fn load_session_tree(&mut self) {
        let Some(session) = self.state.session else {
            return;
        };
        match self.store.session_tree(session).await {
            Ok(tree) => {
                self.state.tree = Arc::new(tree);
                self.publish_snapshot();
            }
            Err(error) => self.note_store_failure(&error),
        }
    }

    /// Follow the branch ending at `sequence` again.
    ///
    /// The counterpart to [`rewind_to`](Self::rewind_to): that one steps *off*
    /// a branch, this one steps back *onto* one. Both are a move of the active
    /// leaf followed by a re-read, and neither writes an event — the branch
    /// already exists, and returning to it is not a new fact about the session.
    ///
    /// Unlike a rewind, the leaf lands on `sequence` itself rather than its
    /// parent: "go back to this branch" means the turn selected is part of what
    /// you are returning to, where "branch from this message" means it is not.
    pub(super) async fn follow_branch(&mut self, sequence: u64) {
        let Some(session) = self.state.session else {
            return;
        };
        if self.run.is_some() {
            return;
        }
        let left = self.summarise_current_branch(session).await;
        if let Err(error) = self.store.set_active_leaf(session, Some(sequence)).await {
            self.note_store_failure(&error);
            return;
        }
        let events = match self.store.branch_events(session).await {
            Ok(events) => events,
            Err(error) => {
                self.note_store_failure(&error);
                return;
            }
        };
        if let Err(error) = self.state.rebuild_messages_from(&events) {
            self.note_store_failure(&error);
            return;
        }
        self.state.left_branch = left;
        // Following an existing branch continues it. Only a rewind creates a
        // branch point, so nothing is pending here.
        self.state.branch_parent = None;
        // The tree the user is looking at was just made stale by this move:
        // every node's `on_active_branch` may have flipped.
        if let Ok(tree) = self.store.session_tree(session).await {
            self.state.tree = Arc::new(tree);
        }
        self.publish_snapshot();
    }

    /// Re-read skills, prompt templates, and project instructions from disk
    /// .
    ///
    /// A reload that fails leaves the previous resources live and says so: the
    /// alternative is a session that silently loses its skills because someone
    /// broke a YAML file in another terminal.
    pub(super) fn reload_resources(&mut self) {
        let report = match self.context.reload() {
            Ok(reloaded) => {
                let changes = reloaded.changes_since(&self.context);
                let report = crate::core::context::ReloadReport {
                    skills: reloaded.skills().len(),
                    prompts: reloaded.prompts().templates().len(),
                    changes,
                    failure: None,
                };
                self.context = reloaded;
                report
            }
            Err(error) => crate::core::context::ReloadReport {
                skills: self.context.skills().len(),
                prompts: self.context.prompts().templates().len(),
                changes: Vec::new(),
                failure: Some(error.to_string()),
            },
        };
        self.state.last_reload = Some(report);
        self.publish_snapshot();
    }

    /// Register a discovered extension into the live tool registry and record
    /// the load. The one place the registration happens, shared
    /// by the human `/load-extension` command and the model's `load_extension`
    /// tool so the two cannot diverge on what "loaded" means.
    ///
    /// The `ExtensionLoaded` event is written *before* the tool is added, so the
    /// log always reads authorise-then-enable. A store failure propagates so the
    /// caller decides how to surface it; a fresh load is the only case that
    /// records anything.
    async fn register_extension(
        &mut self,
        session: SessionId,
        name: &str,
        by: crate::core::event::ExtensionLoadAuthority,
    ) -> Result<Registration, StoreError> {
        // A built-in or an already-loaded extension owns the name; `add` would
        // silently no-op, so report the collision instead of pretending.
        if self.tools.get(name).is_some() {
            return Ok(Registration::AlreadyAvailable);
        }
        let catalog = self.context.extension_catalog();
        let Some(definition) = catalog.get(name) else {
            return Ok(Registration::NotFound);
        };
        let program = definition.program().to_owned();
        let tool = Arc::new(crate::tools::ExtensionTool::new(definition.clone()));
        self.persist(MjolnrEvent::ExtensionLoaded {
            session,
            name: name.to_owned(),
            program: program.clone(),
            by,
        })
        .await?;
        self.tools.add(tool);
        Ok(Registration::Loaded(program))
    }

    /// The human `/load-extension` command. Inert until this
    /// explicit act, which is itself the authorisation: the file was written
    /// through the `Write` gate, and every call the loaded tool makes is gated
    /// at `Execute` tier, so loading only makes it *callable*, never
    /// auto-approved.
    pub(super) async fn load_extension(&mut self, name: String) {
        // Adding a tool mid-run would change what the model sees inside a turn
        // already underway. A load is a between-turns act, like a policy change.
        if self.run.is_some() {
            return;
        }
        let Some(session) = self.state.session else {
            self.report_extension_load(
                name,
                None,
                Some("open a session before loading an extension"),
            );
            return;
        };
        match self
            .register_extension(
                session,
                &name,
                crate::core::event::ExtensionLoadAuthority::Command,
            )
            .await
        {
            Ok(Registration::Loaded(program)) => {
                self.report_extension_load(name, Some(program), None);
            }
            Ok(Registration::AlreadyAvailable) => {
                let detail = format!("a tool named `{name}` is already available");
                self.report_extension_load(name, None, Some(&detail));
            }
            Ok(Registration::NotFound) => {
                let detail = format!("no discovered extension named `{name}`");
                self.report_extension_load(name, None, Some(&detail));
            }
            Err(error) => self.note_store_failure(&error),
        }
    }

    /// Register an extension the model proposed through the `load_extension`
    /// tool, after that call passed the policy gate and completed (plan
    /// §Phase 17). The tool validated the name and the gate authorised it; here
    /// the actor performs the registration under the authority resolved at the
    /// Execute gate. Runs mid-turn, so the newly registered tool is visible on
    /// the next model call of the same run.
    pub(super) async fn agent_load_extension(
        &mut self,
        session: SessionId,
        run: crate::core::event::RunId,
        name: &str,
        authority: crate::core::event::ExtensionLoadAuthority,
    ) {
        if let Err(error) = self.register_extension(session, name, authority).await {
            self.fail_store(run, &error);
        }
    }

    fn report_extension_load(
        &mut self,
        name: String,
        program: Option<String>,
        failure: Option<&str>,
    ) {
        self.state.last_extension_load = Some(crate::core::context::ExtensionLoadReport {
            name,
            loaded_program: program,
            failure: failure.map(str::to_owned),
        });
        self.publish_snapshot();
    }

    pub(super) async fn set_policy(&mut self, mode: PolicyMode) {
        if self.run.is_some() {
            return;
        }
        if let Some(session) = self.state.session
            && let Err(error) = self
                .persist(MjolnrEvent::PolicyChanged { session, mode })
                .await
        {
            self.note_store_failure(&error);
            return;
        }
        self.state.policy = mode;
        self.publish_snapshot();
    }

    /// Create the durable session, take its lease, and record its opening.
    pub(super) async fn create_session(
        &mut self,
        provider: crate::core::model::ProviderId,
        model: crate::core::model::ModelId,
    ) {
        if self.state.session.is_some() || self.lease.is_some() {
            self.note_store_failure(&StoreError::Unavailable {
                detail: "end the open session before creating another one".to_owned(),
            });
            return;
        }
        if self.state.store_failure.is_some() {
            self.publish_snapshot();
            return;
        }

        // Every session references a project , so a session without one
        // is not a thing that can exist. Refusing here beats letting the foreign
        // key refuse later, where the message would name a constraint rather
        // than the missing step.
        let Some(root) = self.state.workspace_root.clone() else {
            self.note_store_failure(&StoreError::Unavailable {
                detail: "open a project before creating a session".to_owned(),
            });
            return;
        };
        // Never wider than the session this one continues.
        // Applied here, at the one door into a new session, so a fork, a clone,
        // and a new-session-from-handoff cannot disagree about it.
        let initial_policy = self.state.policy.carried_forward();
        self.state.reset_keeping_project();
        // Loaded after the reset: `reset_keeping_project` replaces the whole
        // session state, so a load before it was wiped before any session saw
        // it — the rules snapshot is per-session and must land on the fresh
        // state.
        self.load_rules_snapshot(root.clone()).await;
        // Applied before the new session id becomes visible, not after the
        // durable append below.
        //
        // `reset_keeping_project` puts the policy back to the default, which is
        // *wider* than what is being carried forward. Restoring it only after
        // `persist(SessionCreated)` left a window in which that append's
        // snapshot showed the new session at the default policy — a client
        // observing a narrowed policy widen itself for an instant, which is the
        // one thing AGENTS.md §11.4 does not permit a new session to do. State
        // now leads the durable record here, and that direction is deliberate:
        // if the append below fails, the session is left at the *narrower*
        // policy, which fails closed.
        self.state.policy = initial_policy;
        // A subagent host takes the identity its parent minted, so the durable
        // spawn event, the worktree name, and this transcript all agree
        // . Everyone else mints a fresh one.
        let session = self
            .child_link
            .map_or_else(SessionId::new, |link| link.session);

        let project = match self.store.open_project(root).await {
            Ok(project) => project,
            Err(error) => return self.note_store_failure(&error),
        };

        let title = default_title(&self.state);
        // A subagent host records which session spawned it; a session a human
        // opened has no parent.
        let parent = self.child_link.map(|link| link.parent);
        if let Err(error) = self
            .store
            .create_session(session, project, title, parent)
            .await
        {
            return self.note_store_failure(&error);
        }

        // Taken before the first append: the lease is what stops a second
        // process interleaving its run into this transcript
        // (`docs/persistence.md` §5).
        match self.store.acquire_session(session).await {
            Ok(lease) => self.lease = Some(lease),
            Err(error) => return self.note_store_failure(&error),
        }

        self.state.session = Some(session);
        self.state.provider = Some(provider.clone());
        self.state.model = Some(model.clone());
        self.state.budget = crate::core::runtime::BudgetStatus {
            max_provider_turns: self.limits.max_provider_turns,
            max_tool_calls: self.limits.max_tool_calls,
            ..crate::core::runtime::BudgetStatus::default()
        };
        self.recovery = RecoveryState::Clean;

        if let Err(error) = self
            .persist(MjolnrEvent::SessionCreated {
                session,
                provider,
                model,
            })
            .await
        {
            self.note_store_failure(&error);
            return;
        }
        // The durable record of the narrowing, appended after the session it
        // refers to exists. State already carries it (see above), so this
        // append records the policy rather than establishing it.
        if initial_policy != PolicyMode::default()
            && let Err(error) = self
                .persist(MjolnrEvent::PolicyChanged {
                    session,
                    mode: initial_policy,
                })
                .await
        {
            self.note_store_failure(&error);
            return;
        }
        self.refresh_session_list().await;
    }

    /// Rebuild a session from the latest checkpoint plus every later event.
    #[allow(
        clippy::cognitive_complexity,
        reason = "resume is one fail-closed transaction whose lease cleanup must stay visible"
    )]
    pub(super) async fn resume_session(&mut self, session: SessionId) {
        if self.run.is_some() || self.state.session.is_some() || self.lease.is_some() {
            self.note_store_failure(&StoreError::Unavailable {
                detail: "end the open session before resuming another one".to_owned(),
            });
            return;
        }

        let summary = match self.session_summary(session).await {
            Ok(summary) => summary,
            Err(error) => return self.note_store_failure(&error),
        };
        if summary.status == SessionStatus::Ended {
            self.note_store_failure(&StoreError::Unavailable {
                detail: format!("session {session} has ended and cannot accept new work"),
            });
            return;
        }

        match self.store.acquire_session(session).await {
            Ok(lease) => self.lease = Some(lease),
            Err(error) => return self.note_store_failure(&error),
        }

        let checkpoint = match self.store.latest_checkpoint(session).await {
            Ok(checkpoint) => checkpoint,
            Err(error) => return self.resume_failed(&error).await,
        };

        // A checkpoint covers a *count* of events, so this reads exactly what it
        // does not cover. With no checkpoint, `from` is 0 and the whole branch
        // replays.
        //
        // The read is branch-aware : a session resumes the
        // branch it was left on, not the linear history, so a rewind survives a
        // restart. `None` means the checkpoint covers events this branch left
        // behind — it describes a sibling's transcript, so it is dropped and the
        // branch replays from the beginning rather than being layered onto a
        // history the user branched away from.
        let from = checkpoint.as_ref().map_or(0, |stored| stored.sequence);
        let (checkpoint, resume) = match self.store.branch_events_from(session, from).await {
            Ok(Some(resume)) => (checkpoint, resume),
            Ok(None) => match self.store.branch_events(session).await {
                Ok(events) => (
                    None,
                    crate::core::store::BranchResume {
                        covered_message_sequences: Vec::new(),
                        events,
                    },
                ),
                Err(error) => return self.resume_failed(&error).await,
            },
            Err(error) => return self.resume_failed(&error).await,
        };

        let mut recovered = match recovery::project(
            checkpoint.map(|stored| stored.checkpoint),
            &resume.covered_message_sequences,
            &resume.events,
        ) {
            Ok(recovered) => recovered,
            Err(error) => {
                return self
                    .resume_failed(&StoreError::Decode {
                        detail: error.to_string(),
                    })
                    .await;
            }
        };

        // Checkpoints intentionally carry no plan approval authority — and no
        // decision-ticket state either. Rebuild the client-visible workflow
        // and the decision records from the complete append-only branch so a
        // clean checkpoint cannot make both clients forget an approved plan or
        // a recorded judgement.
        let plan_events = match self.store.branch_events(session).await {
            Ok(events) => events,
            Err(error) => return self.resume_failed(&error).await,
        };
        if let Err(error) = recovered.state.rebuild_durable_records_from(&plan_events) {
            return self.resume_failed(&error).await;
        }

        // Resolved before the projection replaces state wholesale. A session
        // that crashed before its first checkpoint has no root in its
        // checkpoint, and taking the projection's `None` would leave a resumed
        // session with no workspace — every repository tool then refuses, which
        // looks like mjolnr forgetting the project rather than the bug it is.
        let root = match self
            .session_root(&summary, recovered.state.workspace_root.clone())
            .await
        {
            Ok(root) => root,
            Err(error) => return self.resume_failed(&error).await,
        };

        if recovered.status == SessionStatus::Ended {
            return self
                .resume_failed(&StoreError::Unavailable {
                    detail: format!("session {session} has ended and cannot accept new work"),
                })
                .await;
        }

        self.state = recovered.state;
        self.state.session = Some(session);
        self.state.workspace_root = Some(root);
        // Limits are configuration, not history: a rebuilt session obeys this
        // build's budgets, not the ones in force when it was written.
        self.state.budget.max_provider_turns = self.limits.max_provider_turns;
        self.state.budget.max_tool_calls = self.limits.max_tool_calls;
        self.recovery = recovered.recovery.clone();

        self.state.resume_advice = resume_advice(&summary, &self.state);

        // Recorded durably so the transcript says why mjolnr stopped, rather than
        // showing a gap followed by a decision.
        if let RecoveryState::Required(work) = &recovered.recovery
            && let Err(error) = self
                .persist(MjolnrEvent::RecoveryRequired {
                    session,
                    work: Box::new(work.clone()),
                })
                .await
        {
            self.note_store_failure(&error);
        }

        // `self.state = recovered.state` above replaced the repository and
        // change views wholesale, so a resumed session would otherwise render
        // "no project" over a project it has open until something unrelated
        // triggered a refresh. Read git here instead, on the trigger that
        // describes what just happened: the root was re-established.
        //
        // This is what makes read-before-edit evidence and review anchors
        // visible after a restart rather than only after the next write (plan
        // §Phase D3).
        //
        // Before `refresh_session_list`, and that order is load-bearing: both
        // publish, and the first published snapshot of a resumed session is the
        // one a client renders. Publishing the restored review threads with no
        // capture beside them would show every note as stale for one frame,
        // because "nothing captured" is correctly not a claim that a note is
        // current.
        self.refresh_repository(crate::core::repository::RefreshTrigger::ProjectOpened)
            .await;

        self.refresh_session_list().await;
    }

    /// Explicit compact open. The projection is selected before the TUI can
    /// submit a prompt, and an optional model transition is recorded normally.
    pub(super) async fn resume_compact(
        &mut self,
        session: SessionId,
        provider: Option<crate::core::model::ProviderId>,
        model: Option<crate::core::model::ModelId>,
    ) {
        self.resume_session(session).await;
        if self.state.session != Some(session) {
            return;
        }
        match (provider, model) {
            (Some(provider), Some(model)) => {
                if !self.select_model(provider, model).await {
                    return;
                }
            }
            (None, None) => {}
            _ => {
                self.note_store_failure(&StoreError::Unavailable {
                    detail: "compact cross-model resume requires both provider and model"
                        .to_owned(),
                });
                return;
            }
        }
        if !self.state.enable_compact_context(2) {
            self.note_store_failure(&StoreError::Unavailable {
                detail: "compact resume requires a durable handoff; run /handoff first".to_owned(),
            });
            return;
        }
        self.state.resume_advice = None;
        self.publish_snapshot();
    }

    async fn session_summary(&self, session: SessionId) -> Result<SessionSummary, StoreError> {
        self.store
            .sessions()
            .await?
            .into_iter()
            .find(|summary| summary.id == session)
            .ok_or(StoreError::UnknownSession { session })
    }

    /// Revalidate the stored project identity before restoring it.
    ///
    /// Both the project row and checkpoint must name the same canonical path,
    /// and that path must still canonicalize to itself. A directory replaced by
    /// a symlink is therefore refused instead of silently rebinding the session
    /// to another workspace.
    async fn session_root(
        &self,
        summary: &SessionSummary,
        from_checkpoint: Option<std::path::PathBuf>,
    ) -> Result<std::path::PathBuf, StoreError> {
        if let Some(checkpoint_root) = from_checkpoint
            && checkpoint_root != summary.project_root
        {
            return Err(StoreError::Decode {
                detail: format!(
                    "checkpoint root {} does not match session project {}",
                    checkpoint_root.display(),
                    summary.project_root.display()
                ),
            });
        }

        let expected = summary.project_root.clone();
        let stored = expected.clone();
        let canonical =
            tokio::task::spawn_blocking(move || crate::policy::paths::canonical_root(&stored))
                .await
                .map_err(|error| StoreError::Unavailable {
                    detail: format!("workspace validation task did not complete: {error}"),
                })?
                .map_err(|error| StoreError::Unavailable {
                    detail: error.detail,
                })?;

        if canonical != expected {
            return Err(StoreError::Unavailable {
                detail: format!(
                    "session project {} now resolves to {}; refusing to change its workspace identity",
                    expected.display(),
                    canonical.display()
                ),
            });
        }

        Ok(canonical)
    }

    /// Apply a human's recovery decision, durably.
    pub(super) async fn resolve_recovery(&mut self, decision: RecoveryDecision) {
        let Some(session) = self.state.session else {
            return;
        };
        // Nothing to resolve is not a failure; it is a click on a stale button.
        if !self.recovery.is_required() {
            return;
        }

        if let Err(error) = self
            .persist(MjolnrEvent::RecoveryResolved { session, decision })
            .await
        {
            self.note_store_failure(&error);
            return;
        }

        match decision {
            RecoveryDecision::AbandonAndContinue => {
                // The interrupted work is dropped, never re-run. Its outcome
                // stays unknown: mjolnr does not write a `ToolCompleted` or a
                // `ToolFailed` for it, because it knows neither (`AGENTS.md`
                // §1.4).
                self.recovery = RecoveryState::Clean;
                self.publish_snapshot();
            }
            RecoveryDecision::EndSession => {
                self.recovery = RecoveryState::Clean;
                self.end_session().await;
            }
        }
    }

    /// Mark the session ended and let go of it.
    pub(super) async fn end_session(&mut self) {
        // Ending an active run would erase whether an in-flight effect happened.
        // Cancel first and wait for its terminal event; recovery's explicit
        // EndSession path reaches here with no live run.
        if self.run.is_some() {
            return;
        }

        let Some(session) = self.state.session else {
            return;
        };

        if let Err(error) = self.persist(MjolnrEvent::SessionEnded { session }).await {
            self.note_store_failure(&error);
            return;
        }
        if let Err(error) = self.store.end_session(session).await {
            self.note_store_failure(&error);
            return;
        }
        if let Err(error) = self.checkpoint(SessionStatus::Ended).await {
            self.note_store_failure(&error);
            let _ = self.release_lease().await;
            return;
        }
        if let Err(error) = self.release_lease().await {
            self.note_store_failure(&error);
            return;
        }

        self.recovery = RecoveryState::Clean;
        self.state.reset_keeping_project();
        self.refresh_session_list().await;
    }

    /// Write a checkpoint covering everything appended so far.
    ///
    /// Called after each terminal run and before a clean shutdown.
    pub(super) async fn checkpoint(&self, status: SessionStatus) -> Result<(), StoreError> {
        let Some(session) = self.state.session else {
            return Ok(());
        };

        if self.run.is_some() || self.recovery.is_required() || self.state.store_failure.is_some() {
            return Err(StoreError::Unavailable {
                detail: "refusing to checkpoint unsettled or non-durable session state".to_owned(),
            });
        }

        let checkpoint =
            self.build_checkpoint(session, status)
                .map_err(|error| StoreError::Decode {
                    detail: format!("session state could not be checkpointed: {error}"),
                })?;

        self.store.write_checkpoint(checkpoint).await?;
        Ok(())
    }

    /// Project live state into the durable form.
    ///
    /// `exact_commands` is not read here. It is not an oversight and not a
    /// filter that could be forgotten: [`SessionCheckpoint`] has no field to put
    /// it in (`docs/persistence.md` §6).
    fn build_checkpoint(
        &self,
        session: SessionId,
        status: SessionStatus,
    ) -> Result<SessionCheckpoint, crate::core::error::ToolError> {
        Ok(SessionCheckpoint {
            session,
            status,
            project_root: self.state.workspace_root.clone(),
            provider: self.state.provider.clone(),
            model: self.state.model.clone(),
            messages: self.state.plain_messages(),
            usage: self.state.usage,
            policy: self.state.policy,
            budget: self.state.budget,
            read_set: self.state.read_set.entries()?,
            read_evidence: self.state.read_evidence.values().cloned().collect(),
            review_threads: self.state.review_threads.values().cloned().collect(),
            last_mutation_sequence: self.state.last_mutation_sequence,
            successful_command_evidence: self.state.successful_command_evidence.clone(),
            activated_skills: self.state.activated_skills.iter().cloned().collect(),
            workspace_trusted: self.state.workspace_trusted,
            handoff: self.state.handoff.clone(),
            quota_reserve: self.state.quota_reserve.clone(),
            route: self.state.route.clone(),
        })
    }

    /// The acknowledged shutdown: preserve recovery evidence, flush, release.
    ///
    /// A settled session is checkpointed before the flush. An active,
    /// recovery-blocked, or store-failed session is not checkpointed: folding
    /// its unsettled live state into a checkpoint would erase the event tail
    /// that proves interruption. The lease outlives either path.
    ///
    /// Returning `Ok` means the data is committed — the store's `flush` only
    /// answers once its queue has drained through the connection thread. Anything
    /// less would make "clean shutdown" a claim rather than a fact (`AGENTS.md`
    /// §1.3).
    pub(super) async fn shutdown(&mut self) -> Result<(), StoreError> {
        let interrupted = self.run.is_some();
        if let Some(run) = self.run.take() {
            run.cancel.cancel();
        }

        let existing_failure = self.state.store_failure.clone();
        let checkpoint = if interrupted || self.recovery.is_required() || existing_failure.is_some()
        {
            Ok(())
        } else {
            self.checkpoint(SessionStatus::Active).await
        };
        let flushed = self.store.flush().await;
        let released = self.release_lease().await;

        if let Some(detail) = existing_failure {
            return Err(StoreError::Unavailable { detail });
        }
        checkpoint?;
        flushed?;
        released
    }

    pub(super) async fn release_lease(&mut self) -> Result<(), StoreError> {
        let Some(lease) = self.lease.clone() else {
            return Ok(());
        };
        self.store.release_session(&lease).await?;
        if self.lease.as_ref() == Some(&lease) {
            self.lease = None;
        }
        Ok(())
    }

    async fn resume_failed(&mut self, error: &StoreError) {
        match self.release_lease().await {
            Ok(()) => self.note_store_failure(error),
            Err(release_error) => self.note_store_failure(&StoreError::Unavailable {
                detail: format!(
                    "{error}; additionally failed to release the session lease: {release_error}"
                ),
            }),
        }
    }

    /// Stop live work after a durable append failed, without inventing a
    /// terminal event or checkpointing past the open history.
    pub(super) fn halt_for_store(&mut self, run: crate::core::event::RunId, error: &StoreError) {
        if let Some(active) = self.run.take_if(|active| active.id == run) {
            active.cancel.cancel();
        }
        self.state.pending_approval = None;
        self.note_store_failure(error);
    }

    /// Record that a durable write did not happen.
    ///
    /// The session stops accepting autonomous work: continuing would build on
    /// history the store never accepted, and the difference would surface only
    /// after the next restart, as missing work nobody can explain.
    pub(super) fn note_store_failure(&mut self, error: &StoreError) {
        self.state.store_failure = Some(error.to_string());
        self.publish_snapshot();
    }

    /// Whether autonomous work is currently allowed, and why not.
    pub(super) fn blocked(&self) -> Option<(ReasonCode, String)> {
        if let Some(detail) = self.state.store_failure.as_ref() {
            return Some((ReasonCode::ToolExecution, detail.clone()));
        }
        if let RecoveryState::Required(work) = &self.recovery {
            return Some((ReasonCode::RecoveryRequiresDecision, work.summary()));
        }
        if self.state.resume_advice.is_some() {
            return Some((
                ReasonCode::RunActive,
                "choose compact, new-from-handoff, or full resume before continuing".to_owned(),
            ));
        }
        None
    }

    /// Critical ordering seam: persist before broadcasting or beginning an
    /// approved side effect.
    pub(super) async fn persist(&mut self, event: MjolnrEvent) -> Result<StoredEvent, StoreError> {
        let stored = self.store.append(event.clone()).await?;
        self.state
            .apply_event(&event)
            .map_err(|error| StoreError::Decode {
                detail: format!("durable event reduction failed after validated append: {error}"),
            })?;
        self.broadcast(event);
        self.publish_snapshot();
        Ok(stored)
    }

    /// Persist the first event of a new branch, then clear the pending branch
    /// point so everything after it continues linearly.
    pub(super) async fn persist_branching(
        &mut self,
        event: MjolnrEvent,
    ) -> Result<StoredEvent, StoreError> {
        let Some(parent) = self.state.branch_parent.take() else {
            return self.persist(event).await;
        };
        let stored = self.store.append_after(event.clone(), parent).await?;
        self.broadcast(event);
        Ok(stored)
    }

    /// Broadcast without storing. Ephemeral render traffic only.
    pub(super) fn broadcast(&self, event: MjolnrEvent) {
        // An error means nobody is subscribed, which is normal for a headless
        // run. Nothing is lost: durable events are already in the store.
        let _ = self.events.send(event);
    }

    pub(super) fn publish_snapshot(&self) {
        let mut snapshot = self
            .state
            .snapshot(self.run.is_some(), self.recovery.clone());
        snapshot.skills = self.context.skills_arc();
        snapshot.prompts = self.context.prompt_summaries();
        snapshot.extensions = self.context.extension_summaries_arc();
        snapshot.plugins = self.context.plugin_summaries_arc();
        snapshot.last_discovery.clone_from(&self.last_discovery);
        snapshot.context_diagnostics = self.context.diagnostics_arc();
        snapshot.mcp_servers = self.mcp_servers.clone();
        snapshot.triggers = Arc::clone(&self.triggers);

        let mut current_models = self
            .model_catalogs
            .values()
            .flatten()
            .cloned()
            .map(|descriptor| crate::core::runtime::ModelChoice { descriptor })
            .collect::<Vec<_>>();
        current_models.sort_by(|left, right| {
            left.descriptor
                .provider
                .as_str()
                .cmp(right.descriptor.provider.as_str())
                .then_with(|| {
                    left.descriptor
                        .id
                        .as_str()
                        .cmp(right.descriptor.id.as_str())
                })
        });
        snapshot.models = Arc::new(current_models);
        let mut providers = self
            .provider_connections
            .values()
            .cloned()
            .collect::<Vec<_>>();
        providers.sort_by(|left, right| left.provider.as_str().cmp(right.provider.as_str()));
        snapshot.providers = Arc::new(providers);
        snapshot.routes = Arc::new(self.route_choices());
        snapshot.personas = self.context.persona_summaries();
        self.state
            .persona_override
            .clone_into(&mut snapshot.active_persona);
        snapshot.souls = self.context.soul_files();
        snapshot.external_agents = self.external_agents.views();
        snapshot.external_agent_capability = {
            let available = self.state.workspace_root.is_some();
            crate::core::client::external_agent::ExternalAgentCapability {
                available,
                reason: if available {
                    None
                } else {
                    Some("no project is open".to_owned())
                },
            }
        };

        // Failure means every receiver is gone, i.e. shutdown.
        let _ = self.snapshot.send(snapshot);
    }

    /// The project's routes as selectable choices for a `/route`/`/role`
    /// picker. A route with no first hop is omitted rather than offered — it
    /// is one `AttachRoute` would no-op on, and offering a choice that does
    /// nothing would be the kind of quiet lie §1.3 forbids.
    fn route_choices(&self) -> Vec<crate::core::runtime::RouteChoice> {
        self.route_table
            .routes
            .values()
            .filter_map(|definition| {
                let hop = definition.hop(0)?;
                Some(crate::core::runtime::RouteChoice {
                    name: definition.name.clone(),
                    roles: definition.roles.clone(),
                    provider: hop.provider.clone(),
                    model: hop.model.clone(),
                    persona: definition.persona.clone(),
                })
            })
            .collect()
    }
}

fn resume_advice(summary: &SessionSummary, state: &SessionState) -> Option<ResumeAdvice> {
    let warning = if state.quota_reserve.phase == QuotaReservePhase::Stopped {
        Some(ResumeWarning::QuotaStopped {
            resets_at: state.quota_reserve.resets_at,
        })
    } else {
        let idle = time::OffsetDateTime::now_utc() - summary.updated_at;
        let seconds = u64::try_from(idle.whole_seconds()).unwrap_or(0);
        (seconds >= 24 * 60 * 60).then_some(ResumeWarning::Stale {
            idle_seconds: seconds,
        })
    }?;
    Some(ResumeAdvice {
        warning,
        estimated_full_resume_tokens: estimate_context_tokens(state.messages()),
        handoff: state.handoff.as_ref().map(|handoff| handoff.id),
    })
}

fn estimate_context_tokens(messages: &[crate::core::message::TranscriptEntry]) -> u64 {
    let characters = messages
        .iter()
        .map(|message| message.text().chars().count())
        .sum::<usize>();
    u64::try_from(characters.div_ceil(4)).unwrap_or(u64::MAX)
}
