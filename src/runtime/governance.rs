//! Applying the declared per-model governance floor ( part A).
//!
//! [`crate::governance`] reads the file and [`crate::core::governance`] says
//! what a tier means. Neither of them applies anything — this is where a tier
//! becomes a clamp, at the same doors every other ceiling is enforced at.

use crate::core::event::SmedEvent;
use crate::core::governance::GovernanceTable;
use crate::core::policy::PolicyMode;

use super::Actor;

impl Actor {
    /// The project's declared table, read from disk.
    ///
    /// Read fresh rather than cached, on the same reasoning
    /// `context::harness` records for workspace facts: this is one small file
    /// read at two low-frequency doors, against a cached value that could go
    /// stale mid-session and lie. Here the lie is worse than a stale branch
    /// name — an owner who tightens `governance.yaml` because a model is
    /// misbehaving would not get the tightening until restart, and smed
    /// would go on running at authority the file no longer grants.
    fn governance_table(&self) -> GovernanceTable {
        self.state
            .workspace_root
            .as_ref()
            .map_or_else(GovernanceTable::default, |root| {
                // Diagnostics are dropped the way `bind_route_persona` drops
                // the routing loader's: the table a bad file resolves to is
                // already the narrow one, so the session is safe while the
                // human reads their own file.
                let (table, _diagnostics) = crate::governance::load_dir(root);
                table
            })
    }

    /// Narrow the session's policy to what the current model is trusted with.
    ///
    /// Returns the policy in force afterwards. A no-op when the tier's ceiling
    /// is already at or above the session's policy, which is the common case
    /// and costs one file read.
    ///
    /// Recorded, never silent. A session that quietly stopped being full-auto
    /// would be lying about its own state (`AGENTS.md` §1.3) — the same
    /// reasoning that makes law 6's external-directive cap a recorded act
    /// rather than an applied one.
    pub(super) async fn apply_governance_floor(&mut self) -> PolicyMode {
        let (Some(session), Some(provider), Some(model)) = (
            self.state.session,
            self.state.provider.clone(),
            self.state.model.clone(),
        ) else {
            return self.state.policy;
        };

        let tier = self.governance_table().tier_for(&provider, &model);
        let from = self.state.policy;
        let to = tier.clamp(from);
        if to == from {
            return from;
        }

        if let Err(error) = self
            .persist(SmedEvent::PolicyClamped {
                session,
                from,
                to,
                provider,
                model,
                tier,
            })
            .await
        {
            // The record is the run (law 5). If the narrowing cannot be
            // written it has not happened, and continuing at the wider policy
            // while believing otherwise is the failure this whole phase
            // exists to prevent — so the policy is left alone and the store
            // failure is surfaced.
            self.note_store_failure(&error);
            return from;
        }

        self.state.policy = to;
        // An envelope the narrowed policy no longer justifies must not survive
        // it, exactly as it does not survive a human narrowing the policy by
        // hand.
        self.reconcile_envelope_with_policy(to).await;
        self.publish_snapshot();
        to
    }
}
