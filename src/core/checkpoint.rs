//! The durable projection of a session.
//!
//! A checkpoint is written after every terminal run and before a clean shutdown.
//! It is an optimisation and a safety net, never the only truth: recovery is
//! always "latest checkpoint, plus every durable event after it". A mutation
//! that completed after the last checkpoint is recovered from its event, not
//! lost.
//!
//! # What this type deliberately cannot hold
//!
//! There is no field for exact-command approval grants, and that absence is the
//! feature.  scopes `ApproveExactForSession` to one session; a grant
//! that survived a restart would silently widen the authority a human granted —
//! the same class of defect as a blanket approval, arriving through the back
//! door.
//!
//! Enforcing that with a rule ("remember not to serialise `exact_commands`")
//! would last exactly until someone added a field to `SessionState` and mirrored
//! it here by reflex. Enforcing it with the type means restoring a grant
//! requires adding a field, in a diff a reviewer can see (`AGENTS.md` §2.4).

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::core::continuation::{HandoffCheckpoint, QuotaReserveStatus};
use crate::core::event::SessionId;
use crate::core::message::CanonicalMessage;
use crate::core::model::{ModelId, ProviderId, Usage};
use crate::core::policy::PolicyMode;
use crate::core::routing::RouteRuntime;
use crate::core::runtime::BudgetStatus;
use crate::core::store::SessionStatus;

/// Everything about a session that is safe to restore.
///
/// "Safe" is doing real work in that sentence: every field here is either
/// inert data (messages, usage) or a *restriction* (policy, budgets). Nothing
/// here grants authority.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionCheckpoint {
    pub session: SessionId,
    /// Whether the session still accepts work.
    ///
    /// Status is a restriction, not authority. Persisting it prevents a
    /// checkpoint that covers `SessionEnded` from resurrecting the session as
    /// active when there are no later events to replay.
    pub status: SessionStatus,
    /// The canonical root, already resolved. Recovery re-canonicalises rather
    /// than trusting this: the directory may have moved, or become a symlink,
    /// since it was written.
    pub project_root: Option<PathBuf>,
    pub provider: Option<ProviderId>,
    pub model: Option<ModelId>,
    pub messages: Vec<CanonicalMessage>,
    pub usage: Usage,
    pub policy: PolicyMode,
    pub budget: BudgetStatus,
    /// Observed file versions: path → SHA-256.
    ///
    /// Restoring this is what lets a resumed session edit a file it read before
    /// the crash. It is safe *because* it is a restriction: a stale entry causes
    /// a `STALE_FILE_VERSION` refusal, never a silent overwrite. The version is
    /// rechecked against the real file at the side-effect boundary regardless.
    pub read_set: Vec<(PathBuf, String)>,
    /// Which durable tool event recorded each read.
    ///
    /// Beside `read_set` because it is the same fact seen from the other end,
    /// and it has to be here for the same reason: a checkpoint that covers the
    /// read event stops that event being replayed, so evidence rebuilt only
    /// from replay would vanish at exactly the restarts a checkpoint is meant
    /// to make cheap. Inert — it cites history and authorises nothing.
    pub read_evidence: Vec<crate::core::change_capture::ReadRecord>,
    /// Line notes pinned to a diff.
    ///
    /// Here for the same reason `read_evidence` is: a checkpoint that covers a
    /// note's event stops that event being replayed, and §D3 requires notes to
    /// survive a restart with their original anchor. Inert — a note is a human
    /// remark about code, and restoring one authorises nothing.
    pub review_threads: Vec<crate::core::review::ReviewThread>,
    /// Sequence of the most recent completed mutation, for evidence ordering.
    pub last_mutation_sequence: Option<u64>,
    /// Successful `run_command` evidence: call id → the event sequence that
    /// proves it. `finish_task` cites these.
    pub successful_command_evidence: BTreeMap<String, u64>,
    /// Skill names whose full instructions entered canonical context.
    pub activated_skills: Vec<String>,
    /// Whether this session's human approved project-skill instructions.
    ///
    /// This grants no side-effect authority; scripts still cross tool policy.
    pub workspace_trusted: bool,
    /// Latest provider-neutral handoff, if a drain or `/handoff` produced one.
    pub handoff: Option<HandoffCheckpoint>,
    /// Last durable quota boundary state used by the resume advisor.
    pub quota_reserve: QuotaReserveStatus,
    /// This session's live position on an attached route.
    /// Restoring this is what keeps a resumed session on the same hop rather
    /// than silently reverting to the route's first position, or to no route
    /// at all — either would misreport which provider/model the session is
    /// actually continuing on.
    pub route: Option<RouteRuntime>,
}

impl SessionCheckpoint {
    /// An empty checkpoint for a session with no durable history yet.
    #[must_use]
    pub fn empty(session: SessionId) -> Self {
        Self {
            session,
            status: SessionStatus::Active,
            project_root: None,
            provider: None,
            model: None,
            messages: Vec::new(),
            usage: Usage::default(),
            policy: PolicyMode::default(),
            budget: BudgetStatus::default(),
            read_set: Vec::new(),
            read_evidence: Vec::new(),
            review_threads: Vec::new(),
            last_mutation_sequence: None,
            successful_command_evidence: BTreeMap::new(),
            activated_skills: Vec::new(),
            workspace_trusted: false,
            handoff: None,
            quota_reserve: QuotaReserveStatus::default(),
            route: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant of this module, asserted where someone changing it will
    /// read it.
    ///
    /// A field name is a weak signal, so this test is deliberately not the only
    /// guard — `tests/persistence_recovery.rs` proves an approved command does
    /// not survive a restart end-to-end. This one catches the mistake at the
    /// moment it is typed, which is cheaper.
    #[test]
    fn a_checkpoint_cannot_carry_an_approval_grant() {
        let checkpoint = SessionCheckpoint::empty(SessionId::new());
        let rendered = format!("{checkpoint:?}");

        assert!(
            !rendered.contains("exact_command") && !rendered.contains("approval"),
            "a checkpoint must not carry approval authority across a restart; \
             found it in: {rendered}"
        );
    }

    #[test]
    fn an_empty_checkpoint_restores_the_default_policy_not_a_permissive_one() {
        // Fail closed: a checkpoint that lost its policy must not resume into a
        // more permissive mode than the session started in (AGENTS.md §1.2).
        let checkpoint = SessionCheckpoint::empty(SessionId::new());
        assert_eq!(checkpoint.policy, PolicyMode::default());
        assert_ne!(checkpoint.policy, PolicyMode::WorkspaceWrite);
    }
}
