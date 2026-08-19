//! Decision tickets and their resolutions (Phase E5).
//!
//! A decision ticket resolves an unknown; an implementation ticket does work.
//! The two kinds have different terminal states — a decision ticket is done
//! when a *human has judged*, not when work exists — which is why folding them
//! into one type is the exact failure the board design exists to prevent. This module owns only the *decision* kind; imported implementation
//! items arrive through §D6 with their own provenance.
//!
//! The structural contract is ADR-0015's: a resolution is durable human
//! **judgement, never authority**. Three rules this module enforces, stated
//! here once:
//!
//! 1. A resolution is its own type. It is not `PlanApproval`, carries no
//!    `PolicyMode`, and appears in no signature that grants capability.
//! 2. A resolution moves the frontier; it never widens a policy. Decidability
//!    and permission are different questions and, after ADR-0015, different
//!    code paths.
//! 3. Resolutions are permanent and additive. Changing your mind records a new
//!    resolution that supersedes the old one by reference — never a mutation
//!    and never a deletion, because the reasoning behind a superseded decision
//!    is what stops it being re-litigated from scratch.
//!
//! The serde derives exist for one purpose: these are durable records. No
//! client wire depends on them — a separate DTO projects them (the same
//! narrow exception `core::review`'s types carry, per
//! `src/store/wire/mod.rs`).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Identifies one decision ticket permanently and addressably — long after
/// the code around it has changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DecisionTicketId(Uuid);

/// Identifies one resolution. Permanent and additive: superseded resolutions
/// stay addressable because their reasoning is the point of keeping them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DecisionResolutionId(Uuid);

impl DecisionTicketId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Rebuild an id read from durable history or sent by a client.
    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for DecisionTicketId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for DecisionTicketId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl DecisionResolutionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Rebuild an id read from durable history.
    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for DecisionResolutionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for DecisionResolutionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// What kind of unknown the ticket settles. The type shapes what
/// evidence a resolution is expected to carry. **It grants nothing.**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecisionTicketKind {
    /// Settle an unknown by finding things out.
    Research,
    /// Settle an unknown by trying a bounded version of it; running the
    /// prototype is ordinary gated work, not a board feature.
    Prototype,
    /// An interview that settles a question.
    Grilling,
    /// Something to be done in the world once its decision is made.
    Task,
}

/// A question recorded so it can wait: the human's unknown, the options they
/// will choose between, and the tickets that must resolve first.
///
/// There is deliberately no `status` field and no `opened_by`: a ticket's
/// state is its resolution record, and its record lives in the event log —
/// two things a struct field could only shadow (AGENTS.md §11.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionTicket {
    pub id: DecisionTicketId,
    /// The question, verbatim. Bounded at the wire.
    pub question: String,
    pub kind: DecisionTicketKind,
    /// The options a resolution may choose from, in stable order. A decision
    /// with fewer than two options is not a decision, which the wire refuses
    /// rather than records.
    pub options: Vec<String>,
    /// The tickets blocking this one (its blocking-graph in-edges). Edges are
    /// recorded at open time and never mutated: an edge is a fact about
    /// ordering, and a silent re-edging is exactly what would let the board
    /// lie about *why* something became decidable. Cycles are representable —
    /// surfacing them is the frontier's job,
    /// and refusing them here would hide the thing it must name.
    pub blocked_by: Vec<DecisionTicketId>,
}

/// Who authored a resolution. A single-variant enum, on purpose: ADR-0015
/// fixes that judgement has an author *and that a model may never appear in
/// it*. One inhabitable variant is the type-level spelling of that rule; the
/// runtime stamps it when recording, never accepting it from a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecisionAuthor {
    Human,
}

/// Durable human judgement on one ticket (ADR-0015).
///
/// Reads like this deliberately: the `question` and `options` are recorded
/// **verbatim** so a resolution cannot be read out of context — there is no
/// way to hold one and conclude "therefore I may." The chosen option is a
/// **reference** into `options`, never a status word: "the owner chose option
/// B" is a fact about a decision; "this ticket is approved" would be a claim
/// about permission, and that is not a thing a decision ticket is entitled to
/// say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionResolution {
    pub id: DecisionResolutionId,
    pub ticket: DecisionTicketId,
    /// The question as it stood when resolved — verbatim, not by reference:
    /// a resolution read without its question is not evidence of anything.
    pub question: String,
    /// The options as they stood when resolved. The rejected alternatives are
    /// the part a summary loses, and the part that stops a settled decision
    /// being re-argued.
    pub options: Vec<String>,
    /// Index into [`Self::options`].
    pub chosen_option: usize,
    pub decided_by: DecisionAuthor,
    pub decided_at: OffsetDateTime,
    /// The reasoning in the human's words, bounded.
    pub note: Option<String>,
    /// Present only when this replaces an earlier resolution (ADR-0015:
    /// permanent and additive).
    pub supersedes: Option<DecisionResolutionId>,
}

/// One ticket and its current effective resolution, as the runtime holds it.
///
/// This is a working record over the durable log — the log, not this map, is
/// the truth. Ticket state is session-scoped in memory (the same shape
/// `review_threads` takes); permanence comes from the events, and the
/// cross-session projection is the frontier's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionTicketRecord {
    pub ticket: DecisionTicket,
    /// The current effective resolution. Older resolutions stay addressable
    /// in the durable log through `supersedes`; this is the one the frontier
    /// reads.
    pub resolution: Option<DecisionResolution>,
}

impl DecisionTicketRecord {
    /// Apply a resolution after checking it actually resolves *this* ticket.
    ///
    /// The checks are the ADR-0015 constraints as code: the resolution must
    /// quote this ticket's question and options verbatim, its choice must
    /// reference a recorded option, and its supersession must name the
    /// resolution it replaces. Anything else is a bug entering the durable
    /// log — refused, never folded in (AGENTS.md §1.2).
    pub fn apply_resolution(&mut self, resolution: DecisionResolution) -> Result<(), String> {
        if resolution.ticket != self.ticket.id {
            return Err(format!(
                "resolution {} names unknown ticket {}",
                resolution.id, resolution.ticket
            ));
        }
        if resolution.question != self.ticket.question {
            return Err(
                "the resolution's recorded question does not match the ticket's; the record \
                 and its resolution cannot drift"
                    .to_owned(),
            );
        }
        if resolution.options != self.ticket.options {
            return Err(
                "the resolution's recorded options do not match the ticket's; the record \
                 and its resolution cannot drift"
                    .to_owned(),
            );
        }
        if resolution.chosen_option >= self.ticket.options.len() {
            return Err(format!(
                "chosen option {} is not one of the {} recorded options; a resolution must \
                 reference an option considered",
                resolution.chosen_option,
                self.ticket.options.len()
            ));
        }
        match (&self.resolution, resolution.supersedes) {
            (None, None) => {}
            (Some(current), Some(prior)) if prior == current.id => {}
            _ => {
                return Err(
                    "a resolution replacing an earlier one must supersede it by reference, and \
                     a first resolution supersedes nothing; the chain is additive, never a \
                     rewrite"
                        .to_owned(),
                );
            }
        }
        self.resolution = Some(resolution);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket() -> DecisionTicket {
        DecisionTicket {
            id: DecisionTicketId::new(),
            question: "Ship with the queue or without it?".to_owned(),
            kind: DecisionTicketKind::Research,
            options: vec!["with the queue".to_owned(), "without it".to_owned()],
            blocked_by: Vec::new(),
        }
    }

    fn resolution_for(ticket: &DecisionTicket, chosen: usize) -> DecisionResolution {
        DecisionResolution {
            id: DecisionResolutionId::new(),
            ticket: ticket.id,
            question: ticket.question.clone(),
            options: ticket.options.clone(),
            chosen_option: chosen,
            decided_by: DecisionAuthor::Human,
            decided_at: OffsetDateTime::UNIX_EPOCH,
            note: None,
            supersedes: None,
        }
    }

    /// The ADR-0015 constraint, as a test rather than a comment: a resolution
    /// must carry no field an executor can branch on. Serialize and walk the
    /// keys — if a `status`, `approved`, `policy`, or `verdict` ever appears,
    /// this is the build failure that stops it.
    #[test]
    fn a_resolution_carries_no_field_an_executor_can_branch_on() {
        let ticket = ticket();
        let resolution = resolution_for(&ticket, 1);
        let value = serde_json::to_value(&resolution).expect("serialization is total here");

        let object = value.as_object().expect("a resolution is an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "chosen_option",
                "decided_at",
                "decided_by",
                "id",
                "note",
                "options",
                "question",
                "supersedes",
                "ticket"
            ],
            "only ADR-0015's nine fields may exist: {keys:?}"
        );
        // And the chosen option is a reference, never a status word.
        assert_eq!(object["chosen_option"], serde_json::json!(1));
        assert_eq!(object["decided_by"], serde_json::json!("human"));
    }

    #[test]
    fn the_first_resolution_needs_no_chain_and_the_second_needs_it() {
        let ticket = ticket();
        let mut record = DecisionTicketRecord {
            ticket: ticket.clone(),
            resolution: None,
        };

        let first = resolution_for(&ticket, 0);
        let error = record
            .apply_resolution(DecisionResolution {
                supersedes: Some(DecisionResolutionId::new()),
                ..first.clone()
            })
            .expect_err("superseding nothing must refuse");
        assert!(error.contains("supersede"));

        record
            .apply_resolution(first.clone())
            .expect("a first resolution applies");

        let error = record
            .apply_resolution(resolution_for(&ticket, 1))
            .expect_err("replacing without the chain must refuse");
        assert!(error.contains("supersede"));

        record
            .apply_resolution(DecisionResolution {
                supersedes: Some(first.id),
                ..resolution_for(&ticket, 1)
            })
            .expect("a properly chained replacement applies");
    }

    #[test]
    fn a_resolution_of_a_different_question_is_refused() {
        let ticket = ticket();
        let mut record = DecisionTicketRecord {
            ticket: ticket.clone(),
            resolution: None,
        };

        let mut wrong = resolution_for(&ticket, 0);
        "a different question entirely".clone_into(&mut wrong.question);
        let error = record.apply_resolution(wrong).expect_err("must refuse");
        assert!(error.contains("question"));

        let mut out_of_range = resolution_for(&ticket, ticket.options.len());
        out_of_range.question.clone_from(&ticket.question);
        let error = record
            .apply_resolution(out_of_range)
            .expect_err("must refuse an unrecorded option");
        assert!(error.contains("recorded options"));

        let mut moved_options = resolution_for(&ticket, 0);
        let slot = moved_options.options.first_mut().expect("two options");
        "Option C".clone_into(slot);
        let error = record
            .apply_resolution(moved_options)
            .expect_err("must refuse moved options");
        assert!(error.contains("options"));
    }
}
