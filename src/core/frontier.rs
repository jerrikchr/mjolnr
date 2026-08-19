//! The frontier: what is decidable right now (Phase E5, step 2).
//!
//! The frontier is a deterministic function over
//! the blocking graph — a property of recorded state, not a model's opinion.
//! Every node lands in exactly one of three sets: `frontier` (unresolved, no
//! unresolved in-edges), `fog` (unresolved, at least one unresolved in-edge),
//! and `settled` (resolved). Blocking cycles are surfaced, never broken: a
//! member of a cycle is fogged behind the other members, and the board names
//! the cycle. This module is that function, with the five properties from
//! §3 tested directly in `tests/frontier.rs`:
//!
//! - **Pure** — the same recorded state in produces the same board out. There
//!   is no clock, no model call, no randomness, and no ordering except the
//!   ids' total order.
//! - **Total** — every input node lands in exactly one of the three sets; the
//!   classification is exhaustive, and an unclassifiable node would be a bug
//!   in this module, never a silent fourth bucket.
//! - **Cycles surfaced, not broken** — nothing in a blocking cycle is
//!   decidable; the board names the cycle, and only a resolution — a human
//!   judgement (ADR-0015) — legitimately unblocks.
//! - **Provenance survives** — every projected entry carries its source
//!   (`Provenance`): the `smedGoverned` records this step projects, and the
//!   `externalUnverified` items §D6 will add in step 4, never mixed silently.
//!   `Provenance` is a core notion; the wire DTO `TrustClass`
//!   (`core::client::workspace`, ADR 0006) is a bridge concern and is mapped
//!   from it at projection.
//! - **Shows its working** — a fogged node names the unresolved blockers it
//!   waits on, and only the unresolved ones: a resolved blocker is no longer
//!   a reason.
//!
//! Step-4 input, per the design's build order (§5): decision tickets, plan
//! state, and imported work items. The frontier already carried a
//! `Provenance::ExternalUnverified` variant reserved for this step; it is now
//! produced. Imported items are implementation nodes (`NodeKind::Implementation`)
//! under design §2 — "done when work exists" — and settle only on an observed
//! terminal outcome (`Closed | Merged | Done`). The plan family maps under the
//! design's §6 default — one node per plan, judged by its active stage — with
//! the settled boundary stated explicitly: a plan is resolved once a human has
//! judged it (`Approved`, `Rejected`) or it has entered execution (`Handoff`).
//! `Idle`, `QuestionPending`, `Proposed`, `Reviewed`, and `IterateRequested` are
//! still in play. Imported items are the third implementation source alongside
//! plans (design §2, ADR-0014).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::core::board::{DecisionTicketId, DecisionTicketRecord};
use crate::core::imported::{ImportedItem, ImportedItemId};
use crate::core::plan::{PlanId, PlanStage, PlanWorkflow};

/// One node on the board: a decision ticket, a plan, or an imported item.
///
/// The id is the board's address for the node; its total order is also the
/// frontier's only tie-breaker, which is what keeps the projection pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeId {
    /// A decision ticket: settles an unknown when a human judges it.
    Decision(DecisionTicketId),
    /// A plan: does work; resolved once its fate is fixed.
    Plan(PlanId),
    /// A work item imported from an external tracker (GitHub, Linear — §D6).
    /// `externalUnverified` (§2 provenance); "done when work exists" (§2).
    Imported(ImportedItemId),
}

impl NodeId {
    /// What this node settles: an unknown or a piece of work.
    #[must_use]
    pub const fn kind(self) -> NodeKind {
        match self {
            NodeId::Decision(_) => NodeKind::Decision,
            NodeId::Plan(_) | NodeId::Imported(_) => NodeKind::Implementation,
        }
    }

    /// Where this node's record comes from. `Decision` and `Plan` are
    /// `SmedGoverned`; `Imported` is `ExternalUnverified` (design §2,
    /// ADR-0014, §D6). Every entry the frontier emits carries this.
    #[must_use]
    pub const fn provenance(self) -> Provenance {
        match self {
            NodeId::Decision(_) | NodeId::Plan(_) => Provenance::SmedGoverned,
            NodeId::Imported(_) => Provenance::ExternalUnverified,
        }
    }
}

/// What a node settles: an unknown or work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Settles an unknown; done when a human has judged (ADR-0015).
    Decision,
    /// Does work; done when the work exists.
    Implementation,
}

/// Where a node's record comes from — its provenance.
///
/// This is the core notion; the wire DTO `TrustClass`
/// (`core::client::workspace`) is ADR 0006's bridge concern and must be
/// mapped from this enum at projection, never imported here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Provenance {
    /// Created and judged by smed's own governed process: decision tickets
    /// and the plan family (ADR-0014).
    SmedGoverned,
    /// Arrives from an external tracker (§D6 import, step 4); never mixed
    /// silently with `SmedGoverned` entries.
    ExternalUnverified,
}

/// A node in its current state on the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontierNode {
    /// The node's address; also names the source record it projects.
    pub id: NodeId,
    /// What the node settles.
    pub kind: NodeKind,
    /// The record's provenance; never elided in any of the three sets.
    pub provenance: Provenance,
}

/// A fogged node, answering "why not decidable?" by naming what it waits on
/// (property 5). `waits_on` contains only unresolved blockers: a resolved
/// blocker is no longer a reason, and naming it would make the board lie about
/// why something became decidable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoggedNode {
    /// The node itself, with provenance intact.
    pub node: FrontierNode,
    /// The unresolved blockers this node waits on.
    pub waits_on: BTreeSet<NodeId>,
}

/// The board projection: three disjoint sets covering every node, plus the
/// blocking cycles the frontier refuses to break silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontierBoard {
    /// Decidable now: unresolved, no unresolved in-edges.
    pub frontier: BTreeMap<NodeId, FrontierNode>,
    /// Waiting: unresolved, at least one unresolved in-edge.
    pub fog: BTreeMap<NodeId, FoggedNode>,
    /// Resolved: no longer part of what is decidable.
    pub settled: BTreeMap<NodeId, FrontierNode>,
    /// Cycles among unresolved nodes, each named by its members in id order.
    /// Members also appear in `fog` — nothing in a cycle is decidable — and
    /// this list is the board saying *why*: the cycle is named, not broken
    /// (property 3). A resolved member breaks a cycle; resolution is the only
    /// legitimate unblocking.
    pub cycles: Vec<Vec<NodeId>>,
}

/// Compute the frontier over the blocking graph of decision tickets, plans,
/// and imported items.
///
/// Pure (property 1): the same recorded state in produces the same board out.
/// Total (property 2): every input node lands in exactly one of the three
/// sets, by the exhaustive classification below. Imported items are the third
/// node kind (design §5.4, step 4): the function does not change shape, only
/// its node set does.
#[must_use]
pub fn compute_frontier(
    tickets: &BTreeMap<DecisionTicketId, DecisionTicketRecord>,
    plans: &[PlanWorkflow],
    imported: &BTreeMap<ImportedItemId, ImportedItem>,
) -> FrontierBoard {
    let mut nodes = BTreeMap::new();
    let mut resolved = BTreeSet::new();
    let mut in_edges: BTreeMap<NodeId, BTreeSet<NodeId>> = BTreeMap::new();

    for record in tickets.values() {
        let id = NodeId::Decision(record.ticket.id);
        nodes.insert(
            id,
            FrontierNode {
                id,
                kind: NodeKind::Decision,
                provenance: Provenance::SmedGoverned,
            },
        );
        if record.resolution.is_some() {
            resolved.insert(id);
        }
        for blocker in &record.ticket.blocked_by {
            in_edges
                .entry(id)
                .or_default()
                .insert(NodeId::Decision(*blocker));
        }
    }
    for plan in plans {
        let id = NodeId::Plan(plan.plan_id);
        nodes.insert(
            id,
            FrontierNode {
                id,
                kind: NodeKind::Implementation,
                provenance: Provenance::SmedGoverned,
            },
        );
        if plan_is_settled(plan) {
            resolved.insert(id);
        }
    }
    for item in imported.values() {
        let id = NodeId::Imported(item.id);
        nodes.insert(
            id,
            FrontierNode {
                id,
                kind: NodeKind::Implementation,
                provenance: Provenance::ExternalUnverified,
            },
        );
        if item.state.is_terminal() {
            resolved.insert(id);
        }
        for blocker in &item.blocked_by {
            in_edges.entry(id).or_default().insert(*blocker);
        }
    }

    let unresolved: BTreeSet<NodeId> = nodes
        .keys()
        .copied()
        .filter(|id| !resolved.contains(id))
        .collect();
    let cycles = surface_cycles(&unresolved, &resolved, &in_edges);

    let mut frontier = BTreeMap::new();
    let mut fog = BTreeMap::new();
    let mut settled = BTreeMap::new();
    for (id, node) in nodes {
        if resolved.contains(&id) {
            settled.insert(id, node);
        } else {
            let waits_on: BTreeSet<NodeId> = in_edges
                .get(&id)
                .into_iter()
                .flatten()
                .copied()
                .filter(|blocker| !resolved.contains(blocker))
                .collect();
            if waits_on.is_empty() {
                frontier.insert(id, node);
            } else {
                fog.insert(id, FoggedNode { node, waits_on });
            }
        }
    }

    FrontierBoard {
        frontier,
        fog,
        settled,
        cycles,
    }
}

/// A plan is resolved once its fate is fixed: a human judged it (`Approved`,
/// `Rejected`) or it entered execution (`Handoff`).
fn plan_is_settled(plan: &PlanWorkflow) -> bool {
    matches!(
        plan.stage,
        PlanStage::Approved { .. } | PlanStage::Rejected { .. } | PlanStage::Handoff { .. }
    )
}

/// Name the blocking cycles among unresolved nodes, restricted to edges whose
/// target is unresolved (a resolved target breaks the cycle; that is the
/// legitimate unblocking). A single-node component is not a cycle and is
/// discarded — self-blocking is rejected at record time (`apply_board_event`).
fn surface_cycles(
    unresolved: &BTreeSet<NodeId>,
    resolved: &BTreeSet<NodeId>,
    in_edges: &BTreeMap<NodeId, BTreeSet<NodeId>>,
) -> Vec<Vec<NodeId>> {
    let edges: BTreeMap<NodeId, BTreeSet<NodeId>> = unresolved
        .iter()
        .filter_map(|node| {
            in_edges.get(node).map(|blockers| {
                (
                    *node,
                    blockers
                        .iter()
                        .copied()
                        .filter(|blocker| !resolved.contains(blocker))
                        .collect(),
                )
            })
        })
        .collect();

    let mut searcher = SccSearcher::default();
    let mut cycles: Vec<Vec<NodeId>> = searcher
        .components(unresolved, &edges)
        .into_iter()
        .filter(|component| component.len() > 1)
        .collect();
    cycles.sort_unstable();
    cycles
}

/// Iterative Tarjan strongly-connected-components, seeded by id order so the
/// result is deterministic. Edges are the inverse of the blocker relation:
/// `A blocks B` is stored on B, so following it walks B to its blockers.
#[derive(Default)]
struct SccSearcher {
    next_index: usize,
    indices: BTreeMap<NodeId, usize>,
    lowlinks: BTreeMap<NodeId, usize>,
    stack: Vec<NodeId>,
    on_stack: BTreeSet<NodeId>,
    components: Vec<Vec<NodeId>>,
}

impl SccSearcher {
    /// All components over `nodes`, each sorted, with the components in id
    /// order of their smallest member.
    fn components(
        &mut self,
        nodes: &BTreeSet<NodeId>,
        edges: &BTreeMap<NodeId, BTreeSet<NodeId>>,
    ) -> Vec<Vec<NodeId>> {
        for &node in nodes {
            if !self.indices.contains_key(&node) {
                self.connect(node, edges);
            }
        }
        std::mem::take(&mut self.components)
    }

    fn connect(&mut self, node: NodeId, edges: &BTreeMap<NodeId, BTreeSet<NodeId>>) {
        self.indices.insert(node, self.next_index);
        self.lowlinks.insert(node, self.next_index);
        self.next_index += 1;
        self.stack.push(node);
        self.on_stack.insert(node);

        let successors: Vec<NodeId> = edges.get(&node).into_iter().flatten().copied().collect();
        for successor in successors {
            if !self.indices.contains_key(&successor) {
                self.connect(successor, edges);
                if let Some(candidate) = self.lowlinks.get(&successor).copied() {
                    self.relax_lowlink(node, candidate);
                }
            } else if self.on_stack.contains(&successor)
                && let Some(candidate) = self.indices.get(&successor).copied()
            {
                self.relax_lowlink(node, candidate);
            }
        }

        let is_root = self.lowlinks.get(&node).copied() == self.indices.get(&node).copied();
        if is_root {
            let mut component = Vec::new();
            while let Some(top) = self.stack.pop() {
                self.on_stack.remove(&top);
                component.push(top);
                if top == node {
                    break;
                }
            }
            component.sort_unstable();
            self.components.push(component);
        }
    }

    fn relax_lowlink(&mut self, node: NodeId, candidate: usize) {
        if let Some(low) = self.lowlinks.get_mut(&node)
            && candidate < *low
        {
            *low = candidate;
        }
    }
}

/// A node as a surface renders it: the frontier node plus the human-readable
/// label for the record it projects.
///
/// The label makes the "why is this fogged" answer (property 5) clidable, not
/// just a set of addresses. It is presentation, not decision state: the
/// frontier's classification nevers depends on a label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardNodeView {
    /// The node's address; also names the source record it projects.
    pub id: NodeId,
    /// What the node settles.
    pub kind: NodeKind,
    /// The record's provenance; never elided on a surface either.
    pub provenance: Provenance,
    /// A decision ticket's question or a plan's title. Falls back to the
    /// address when the record has no useful text yet (a plan never proposed).
    pub label: String,
}

/// A fogged node as a surface sees it: the node plus the unresolved blockers
/// that answer "why not decidable?", each with its own label so the board can
/// say *why* in words, not just in ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoggedNodeView {
    /// The node itself, provenance intact.
    pub node: BoardNodeView,
    /// The unresolved blockers `node` waits on, each with its label.
    pub waits_on: Vec<BoardNodeView>,
}

/// The board a surface renders: the pure [`FrontierBoard`] enriched with a
/// label for every node.
///
/// Projection stays out of this module — [`compute_frontier`] is pure and
/// the labels are an enrichment the caller supplies. The overview carries the
/// frontier's three sets and its named cycles verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardOverview {
    /// Imported task records behind external nodes, for clients that need the
    /// exact revision and remote identity to perform a pinned act.
    pub imported_tasks: BTreeMap<ImportedItemId, ImportedItem>,
    /// Durable act history reporting what each submitted change proved or left
    /// ambiguous, keyed by act id so an item's successive acts keep order.
    pub imported_acts:
        BTreeMap<crate::core::imported::ImportedActId, crate::core::imported::ImportedAct>,
    /// Decidable now.
    pub frontier: Vec<BoardNodeView>,
    /// Waiting, each with the unresolved blockers that answer "why not yet".
    pub fog: Vec<FoggedNodeView>,
    /// Resolved.
    pub settled: Vec<BoardNodeView>,
    /// Blocking cycles, each named by its members — including the label each
    /// member carries in `fog`.
    pub cycles: Vec<Vec<BoardNodeView>>,
}

impl BoardOverview {
    /// Enrich a computed frontier with the caller's labels.
    ///
    /// *A* node that appears on the board but has no label — an unresolved
    /// blocker that is not itself a recorded node, for instance — falls back
    /// to its address, so the overview stays total exactly as the frontier is
    /// (property 2).
    #[must_use]
    pub fn from_frontier(board: &FrontierBoard, labels: &BTreeMap<NodeId, String>) -> Self {
        fn node_view(labels: &BTreeMap<NodeId, String>, node: &FrontierNode) -> BoardNodeView {
            BoardNodeView {
                id: node.id,
                kind: node.kind,
                provenance: node.provenance,
                label: labels
                    .get(&node.id)
                    .cloned()
                    .unwrap_or_else(|| render_node_id(node.id)),
            }
        }

        // A blocker that is not itself a recorded node keeps the address's kind
        // and provenance: nothing recorded, so nothing to misattribute.
        fn ref_view(
            labels: &BTreeMap<NodeId, String>,
            id: NodeId,
            all: &BTreeMap<NodeId, &FrontierNode>,
        ) -> BoardNodeView {
            let (kind, provenance) = all
                .get(&id)
                .map_or((NodeKind::Decision, Provenance::SmedGoverned), |node| {
                    (node.kind, node.provenance)
                });
            BoardNodeView {
                id,
                kind,
                provenance,
                label: labels
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| render_node_id(id)),
            }
        }

        let mut all_nodes: BTreeMap<NodeId, &FrontierNode> = BTreeMap::new();
        all_nodes.extend(board.frontier.iter().map(|(id, node)| (*id, node)));
        all_nodes.extend(board.settled.iter().map(|(id, node)| (*id, node)));
        all_nodes.extend(board.fog.iter().map(|(id, fogged)| (*id, &fogged.node)));
        let frontier = board
            .frontier
            .values()
            .map(|node| node_view(labels, node))
            .collect();
        let settled = board
            .settled
            .values()
            .map(|node| node_view(labels, node))
            .collect();
        let fog = board
            .fog
            .values()
            .map(|fogged| FoggedNodeView {
                node: node_view(labels, &fogged.node),
                waits_on: fogged
                    .waits_on
                    .iter()
                    .map(|id| ref_view(labels, *id, &all_nodes))
                    .collect(),
            })
            .collect();
        let cycles = board
            .cycles
            .iter()
            .map(|members| {
                members
                    .iter()
                    .map(|id| ref_view(labels, *id, &all_nodes))
                    .collect()
            })
            .collect();
        Self {
            imported_tasks: BTreeMap::new(),
            imported_acts: BTreeMap::new(),
            frontier,
            fog,
            settled,
            cycles,
        }
    }
}

/// The address a node renders as when no caller-supplied label exists.
fn render_node_id(id: NodeId) -> String {
    match id {
        NodeId::Decision(ticket) => ticket.to_string(),
        NodeId::Plan(plan) => format!("plan {}", plan.as_uuid()),
        NodeId::Imported(imported) => format!("imported {imported}"),
    }
}
