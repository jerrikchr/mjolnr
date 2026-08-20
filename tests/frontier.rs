//! Phase E5 step 2: the frontier computation.
//!
//! The five properties of the frontier computation are tested directly, each
//! as its own test:
//!
//! - **Pure** — the same recorded state in produces the same board out.
//! - **Total** — every node lands in exactly one of the three sets.
//! - **Cycles surfaced, not broken** — a cycle member is fogged, the cycle is
//!   named, and only a resolution unblocks.
//! - **Provenance survives** — every projected entry carries its source and
//!   kind; nothing is elided in any set.
//! - **Shows its working** — a fogged node names the unresolved blockers it
//!   waits on, and only those.
//!
//! Step 2 is deliberately pure: `compute_frontier` takes recorded state and
//! returns the board, with no runtime and no live process. The cross-session
//! projection — reading every session's `DecisionTicket*` and plan events out
//! of the store — is steps 2–3 wiring and belongs with the surface that
//! publishes it, not here.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use mjolnr::core::board::{
    DecisionAuthor, DecisionResolution, DecisionResolutionId, DecisionTicket, DecisionTicketId,
    DecisionTicketKind, DecisionTicketRecord,
};
use mjolnr::core::frontier::{FrontierBoard, NodeId, NodeKind, Provenance, compute_frontier};
use mjolnr::core::plan::{
    PlanApproval, PlanHandoff, PlanId, PlanProposal, PlanStage, PlanWorkflow, ReviewVerdict,
    RevisionId,
};
use time::OffsetDateTime;

fn ticket(question: &str, blocked_by: Vec<DecisionTicketId>) -> DecisionTicket {
    DecisionTicket {
        id: DecisionTicketId::new(),
        question: question.to_owned(),
        kind: DecisionTicketKind::Research,
        options: vec!["with".to_owned(), "without".to_owned()],
        blocked_by,
    }
}

fn resolution_for(ticket: &DecisionTicket) -> DecisionResolution {
    DecisionResolution {
        id: DecisionResolutionId::new(),
        ticket: ticket.id,
        question: ticket.question.clone(),
        options: ticket.options.clone(),
        chosen_option: 0,
        decided_by: DecisionAuthor::Human,
        decided_at: OffsetDateTime::UNIX_EPOCH,
        note: None,
        supersedes: None,
    }
}

fn record(ticket: DecisionTicket, resolved: bool) -> DecisionTicketRecord {
    DecisionTicketRecord {
        resolution: resolved.then(|| resolution_for(&ticket)),
        ticket,
    }
}

fn proposal(plan_id: PlanId) -> PlanProposal {
    PlanProposal {
        plan_id,
        revision_id: RevisionId::initial(),
        title: "the plan".to_owned(),
        summary: "a plan".to_owned(),
        steps: Vec::new(),
        proposed_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn plan(plan_id: PlanId, stage: PlanStage) -> PlanWorkflow {
    PlanWorkflow {
        plan_id,
        interview_goal: None,
        questions: Vec::new(),
        answers: Vec::new(),
        prd: None,
        council_link: None,
        active_revision: Some(RevisionId::initial()),
        stage,
        proposals: Vec::new(),
        reviews: Vec::new(),
        approvals: Vec::new(),
        handoffs: Vec::new(),
    }
}

fn approved_plan(plan_id: PlanId) -> PlanWorkflow {
    plan(
        plan_id,
        PlanStage::Approved {
            proposal: proposal(plan_id),
            approval: PlanApproval {
                plan_id,
                revision_id: RevisionId::initial(),
                approver: "the owner".to_owned(),
                decision: ReviewVerdict::Approve,
                note: None,
                approved_at: OffsetDateTime::UNIX_EPOCH,
            },
        },
    )
}

fn handoff_plan(plan_id: PlanId) -> PlanWorkflow {
    plan(
        plan_id,
        PlanStage::Handoff {
            proposal: proposal(plan_id),
            handoff: PlanHandoff {
                plan_id,
                revision_id: RevisionId::initial(),
                handoff_note: "entered execution".to_owned(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            },
        },
    )
}

fn rejected_plan(plan_id: PlanId) -> PlanWorkflow {
    plan(
        plan_id,
        PlanStage::Rejected {
            proposal: proposal(plan_id),
            reason: "wrong direction".to_owned(),
        },
    )
}

fn tickets(
    records: impl IntoIterator<Item = DecisionTicketRecord>,
) -> BTreeMap<DecisionTicketId, DecisionTicketRecord> {
    records
        .into_iter()
        .map(|record| (record.ticket.id, record))
        .collect()
}

/// Every node the board projects, across all three sets.
fn all_ids(board: &FrontierBoard) -> BTreeSet<NodeId> {
    board
        .frontier
        .keys()
        .chain(board.fog.keys())
        .chain(board.settled.keys())
        .copied()
        .collect()
}

/// The sets partition the input: together they cover every node, pairwise
/// disjoint, and nothing unclassifiable was dropped into a fourth bucket.
fn assert_partition(board: &FrontierBoard, expected: &BTreeSet<NodeId>) {
    assert_eq!(all_ids(board), *expected, "every node lands on the board");
    let mut seen = BTreeSet::new();
    for id in board
        .frontier
        .keys()
        .chain(board.fog.keys())
        .chain(board.settled.keys())
    {
        assert!(seen.insert(*id), "node {id:?} appears in more than one set");
    }
}

#[test]
fn property_pure_identical_state_in_identical_board_out() {
    let a = tickets(vec![
        record(ticket("resolved", Vec::new()), true),
        record(ticket("waiting", Vec::new()), false),
    ]);
    let plans = vec![
        approved_plan(PlanId::new()),
        plan(PlanId::new(), PlanStage::Idle),
    ];

    let first = compute_frontier(&a, &plans, &BTreeMap::new());
    let second = compute_frontier(&a, &plans, &BTreeMap::new());

    assert_eq!(
        first, second,
        "the same recorded state in must produce the same board out: \
         no clock, no model, no ordering except the ids'"
    );
}

#[test]
fn property_total_every_node_in_exactly_one_set() {
    let a = record(ticket("settled by judgement", Vec::new()), true);
    let b = record(ticket("decidable now", Vec::new()), false);
    let c = record(
        ticket("fogged behind b", vec![a.ticket.id, b.ticket.id]),
        false,
    );
    let d = record(ticket("fogged behind c", vec![c.ticket.id]), false);
    let p_frontier = PlanId::new();
    let p_approved = PlanId::new();
    let p_handoff = PlanId::new();
    let p_rejected = PlanId::new();
    let board = compute_frontier(
        &tickets(vec![a.clone(), b.clone(), c.clone(), d.clone()]),
        &[
            plan(
                p_frontier,
                PlanStage::Proposed {
                    proposal: proposal(p_frontier),
                },
            ),
            approved_plan(p_approved),
            handoff_plan(p_handoff),
            rejected_plan(p_rejected),
        ],
        &BTreeMap::new(),
    );

    let expected: BTreeSet<NodeId> = [
        NodeId::Decision(a.ticket.id),
        NodeId::Decision(b.ticket.id),
        NodeId::Decision(c.ticket.id),
        NodeId::Decision(d.ticket.id),
        NodeId::Plan(p_frontier),
        NodeId::Plan(p_approved),
        NodeId::Plan(p_handoff),
        NodeId::Plan(p_rejected),
    ]
    .into_iter()
    .collect();
    assert_partition(&board, &expected);

    let frontier: BTreeSet<NodeId> = board.frontier.keys().copied().collect();
    assert_eq!(
        frontier,
        BTreeSet::from([NodeId::Decision(b.ticket.id), NodeId::Plan(p_frontier)]),
        "only the unresolved, unblocked nodes are decidable"
    );
    let settled: BTreeSet<NodeId> = board.settled.keys().copied().collect();
    assert_eq!(
        settled,
        BTreeSet::from([
            NodeId::Decision(a.ticket.id),
            NodeId::Plan(p_approved),
            NodeId::Plan(p_handoff),
            NodeId::Plan(p_rejected),
        ]),
        "resolved means judged or in execution: approved, handed off, rejected"
    );
    assert!(board.cycles.is_empty(), "no cycles in an acyclic graph");
}

#[test]
fn property_cycles_surfaced_never_broken() {
    let ticket_a = DecisionTicket {
        id: DecisionTicketId::new(),
        question: "a blocks b".to_owned(),
        kind: DecisionTicketKind::Research,
        options: vec!["with".to_owned(), "without".to_owned()],
        blocked_by: Vec::new(),
    };
    let ticket_b = DecisionTicket {
        id: DecisionTicketId::new(),
        question: "b blocks a".to_owned(),
        kind: DecisionTicketKind::Research,
        options: vec!["with".to_owned(), "without".to_owned()],
        blocked_by: vec![ticket_a.id],
    };
    let ticket_a = DecisionTicket {
        blocked_by: vec![ticket_b.id],
        ..ticket_a
    };
    let a = record(ticket_a, false);
    let b = record(ticket_b, false);
    let a_id = a.ticket.id;
    let b_id = b.ticket.id;
    let tickets = tickets(vec![a.clone(), b.clone()]);

    // This state is unconstructible through step-1 commands — a cycle arrives
    // with imported items (§D6) — so it is built directly here, because
    // surfacing it is exactly the frontier's job.
    let board = compute_frontier(&tickets, &[], &BTreeMap::new());

    assert_eq!(board.cycles.len(), 1, "the cycle is named, once");
    let cycle = board.cycles.first().expect("the cycle");
    assert_eq!(cycle.len(), 2);
    assert!(cycle.contains(&NodeId::Decision(a_id)));
    assert!(cycle.contains(&NodeId::Decision(b_id)));

    assert!(!board.frontier.contains_key(&NodeId::Decision(a_id)));
    assert!(!board.frontier.contains_key(&NodeId::Decision(b_id)));
    let fog_a = board.fog.get(&NodeId::Decision(a_id)).expect("a is fogged");
    let fog_b = board.fog.get(&NodeId::Decision(b_id)).expect("b is fogged");
    assert_eq!(fog_a.waits_on, BTreeSet::from([NodeId::Decision(b_id)]));
    assert_eq!(fog_b.waits_on, BTreeSet::from([NodeId::Decision(a_id)]));

    // Resolution is the only legitimate unblocking: resolving a breaks the
    // cycle and makes b decidable. Nothing was erased — a was settled, and
    // the edge from it no longer blocks.
    let mut resolved_a = a;
    resolved_a.resolution = Some(resolution_for(&resolved_a.ticket));
    let mut tickets = tickets;
    tickets.insert(a_id, resolved_a);
    let board = compute_frontier(&tickets, &[], &BTreeMap::new());

    assert!(board.cycles.is_empty(), "a resolution breaks the cycle");
    assert!(board.frontier.contains_key(&NodeId::Decision(b_id)));
    assert!(board.settled.contains_key(&NodeId::Decision(a_id)));
}

#[test]
fn property_provenance_survives_into_every_set() {
    let a = record(ticket("settled", Vec::new()), true);
    let b = record(ticket("decidable", Vec::new()), false);
    let c = record(ticket("fogged", vec![b.ticket.id]), false);
    let p_frontier = PlanId::new();
    let board = compute_frontier(
        &tickets(vec![a, b, c]),
        &[plan(
            p_frontier,
            PlanStage::Proposed {
                proposal: proposal(p_frontier),
            },
        )],
        &BTreeMap::new(),
    );

    for (id, node) in board
        .frontier
        .iter()
        .chain(board.settled.iter())
        .chain(board.fog.iter().map(|(id, fogged)| (id, &fogged.node)))
    {
        assert_eq!(node.id, *id, "the entry names its source");
        assert_eq!(
            node.provenance,
            Provenance::MjolnrGoverned,
            "every step-2 record is mjolnrGoverned, and the class is never elided"
        );
        assert_eq!(
            node.kind,
            match id {
                NodeId::Decision(_) => NodeKind::Decision,
                NodeId::Plan(_) | NodeId::Imported(_) => NodeKind::Implementation,
            }
        );
    }
}

#[test]
fn property_shows_its_working_only_unresolved_blockers_are_named() {
    let a = record(ticket("settled", Vec::new()), true);
    let b = record(ticket("decidable", Vec::new()), false);
    let c = record(ticket("waiting", vec![a.ticket.id, b.ticket.id]), false);
    let d = record(ticket("further back", vec![c.ticket.id]), false);
    let board = compute_frontier(
        &tickets(vec![a, b.clone(), c.clone(), d.clone()]),
        &[],
        &BTreeMap::new(),
    );

    let fog_c = board
        .fog
        .get(&NodeId::Decision(c.ticket.id))
        .expect("c is fogged");
    assert_eq!(
        fog_c.waits_on,
        BTreeSet::from([NodeId::Decision(b.ticket.id)]),
        "the resolved blocker is no longer a reason, and is not named"
    );
    let fog_d = board
        .fog
        .get(&NodeId::Decision(d.ticket.id))
        .expect("d is fogged");
    assert_eq!(
        fog_d.waits_on,
        BTreeSet::from([NodeId::Decision(c.ticket.id)]),
        "the chain names the immediate unresolved blocker"
    );
}
