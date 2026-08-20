//! Phase E5 step 4a: the four contract requirements over imported items.
//!
//! Every board there has `ExternalUnverified` in it somewhere — the §D6
//! provenance — and each test here is a property of the four requirements
//! the E5 contract records:
//!
//! - (a) Revision-pinning — a stale tab is refused, not recorded.
//! - (b) A remote gate is not mjolnr's gate — only observed terminal outcomes
//!   settle; no enforcement-shaped field exists.
//! - (c) Unknown is not zero — `Unknown != Open`, never settled, never cached.
//! - (d) Intersection containment — the frontier's imported nodes are exactly
//!   the input set; a blocker naming an absent id is named, not invented.
//!
//! Two more are the step-2 promises as seen through imported items: cycles
//! arrive via them and are surfaced whole, and provenance survives everywhere.
//!
//! The frontier stays pure: `compute_frontier` takes `&BTreeMap`s and returns
//! the board, with no runtime and no store. The re-fetch distinction (Unknown
//! → Merged settling) is a projection property here — the durable
//! `apply_refresh` guard is tested in `src/core/imported.rs`.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use mjolnr::core::frontier::{NodeId, NodeKind, Provenance, compute_frontier};
use mjolnr::core::imported::{ImportedItem, ImportedItemId, ImportedItemRecord, ImportedItemState};

fn imported(
    id: ImportedItemId,
    revision: &str,
    state: ImportedItemState,
    blocked_by: Vec<NodeId>,
) -> ImportedItem {
    ImportedItem {
        id,
        integration: "github".to_owned(),
        remote_id: "42".to_owned(),
        source_url: "https://example.invalid/42".to_owned(),
        fetched_revision: revision.to_owned(),
        title: "an imported task".to_owned(),
        state,
        blocked_by,
    }
}

fn imported_map(items: Vec<ImportedItem>) -> BTreeMap<ImportedItemId, ImportedItem> {
    items.into_iter().map(|item| (item.id, item)).collect()
}

// ---------------------------------------------------------------------------
// (a) Revision-pinning — the projection reflects the latest fetch; an older
// state's value does not survive as rendered state. The stale-tab refusal
// itself is `ImportedItemRecord::apply_refresh` (tested in core).
// ---------------------------------------------------------------------------

#[test]
fn contract_a_revision_pinning_projects_latest_state_not_cached_forward() {
    let id = ImportedItemId::new();
    let rev1 = imported(id, "rev1", ImportedItemState::Open, Vec::new());
    let board = compute_frontier(&BTreeMap::new(), &[], &imported_map(vec![rev1]));
    assert!(
        board.frontier.contains_key(&NodeId::Imported(id)),
        "rev1 Open is frontier"
    );
    assert!(!board.settled.contains_key(&NodeId::Imported(id)));

    let rev2 = imported(id, "rev2", ImportedItemState::Merged, Vec::new());
    let board = compute_frontier(&BTreeMap::new(), &[], &imported_map(vec![rev2]));
    assert!(
        board.settled.contains_key(&NodeId::Imported(id)),
        "rev2 Merged is settled — the projection reads the supplied record, not a cached prior revision"
    );
    assert!(!board.frontier.contains_key(&NodeId::Imported(id)));

    let mut record =
        ImportedItemRecord::new(imported(id, "rev1", ImportedItemState::Unknown, Vec::new()));
    let stale = imported(id, "rev2", ImportedItemState::Open, Vec::new());
    let refusal = record
        .apply_refresh("not-rev1", stale)
        .expect_err("stale tab must refuse, not record");
    assert_eq!(
        refusal,
        mjolnr::core::imported::RefreshRefusal::StaleRevision {
            expected: "not-rev1".to_owned(),
            current: "rev1".to_owned()
        }
    );
}

// ---------------------------------------------------------------------------
// (b) A remote gate is not mjolnr's gate — only observed terminal outcomes
// settle; no enforcement-shaped field exists on the type.
// ---------------------------------------------------------------------------

#[test]
fn contract_b_a_remote_gate_is_not_smeds_gate() {
    let open = ImportedItemId::new();
    let merged = ImportedItemId::new();
    let closed = ImportedItemId::new();
    let done = ImportedItemId::new();
    let unknown = ImportedItemId::new();
    let board = compute_frontier(
        &BTreeMap::new(),
        &[],
        &imported_map(vec![
            imported(open, "r1", ImportedItemState::Open, Vec::new()),
            imported(merged, "r1", ImportedItemState::Merged, Vec::new()),
            imported(closed, "r1", ImportedItemState::Closed, Vec::new()),
            imported(done, "r1", ImportedItemState::Done, Vec::new()),
            imported(unknown, "r1", ImportedItemState::Unknown, Vec::new()),
        ]),
    );

    assert!(
        board.frontier.contains_key(&NodeId::Imported(open)),
        "Open is not settled"
    );
    assert!(
        board.frontier.contains_key(&NodeId::Imported(unknown)),
        "Unknown is not settled"
    );
    assert!(
        board.settled.contains_key(&NodeId::Imported(merged)),
        "Merged is settled"
    );
    assert!(
        board.settled.contains_key(&NodeId::Imported(closed)),
        "Closed is settled"
    );
    assert!(
        board.settled.contains_key(&NodeId::Imported(done)),
        "Done is settled"
    );

    let item = imported(
        ImportedItemId::new(),
        "r1",
        ImportedItemState::Open,
        Vec::new(),
    );
    let value = serde_json::to_value(&item).expect("an imported item serializes");
    let keys: BTreeSet<String> = value
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    for forbidden in [
        "mergeable",
        "mergeable_state",
        "approved",
        "policy",
        "gate",
        "verdict",
    ] {
        assert!(
            !keys.contains(forbidden),
            "a gate-shaped field '{forbidden}' must not exist on the type"
        );
    }
}

// ---------------------------------------------------------------------------
// (c) Unknown is not zero, and is never cached — `Unknown != Open`, never
// settled, and a re-fetch Unknown → Merged settles the node rather than
// serving the failed read as a checked value.
// ---------------------------------------------------------------------------

#[test]
fn contract_c_unknown_is_not_zero_and_is_never_cached() {
    let id = ImportedItemId::new();
    assert_ne!(ImportedItemState::Unknown, ImportedItemState::Open);
    assert!(!ImportedItemState::Unknown.is_terminal());
    assert!(!ImportedItemState::Open.is_terminal());

    let unknown = imported(id, "rev1", ImportedItemState::Unknown, Vec::new());
    let board = compute_frontier(&BTreeMap::new(), &[], &imported_map(vec![unknown]));
    assert!(
        board.frontier.contains_key(&NodeId::Imported(id)),
        "Unknown is frontier, not settled"
    );
    assert!(!board.settled.contains_key(&NodeId::Imported(id)));

    let merged = imported(id, "rev2", ImportedItemState::Merged, Vec::new());
    let board = compute_frontier(&BTreeMap::new(), &[], &imported_map(vec![merged]));
    assert!(
        board.settled.contains_key(&NodeId::Imported(id)),
        "re-fetching the revision that was Unknown as Merged settles — the failed read was not cached"
    );
}

// ---------------------------------------------------------------------------
// (d) Intersection containment — the frontier's imported nodes are exactly the
// input set; a blocker naming an absent imported id is named in `waits_on`
// (fog), never invented as a node.
// ---------------------------------------------------------------------------

#[test]
fn contract_d_intersection_containment_projects_exactly_the_fetched_set() {
    let a = ImportedItemId::new();
    let b = ImportedItemId::new();
    let absent = ImportedItemId::new();
    let board = compute_frontier(
        &BTreeMap::new(),
        &[],
        &imported_map(vec![
            imported(a, "r1", ImportedItemState::Open, Vec::new()),
            imported(
                b,
                "r1",
                ImportedItemState::Open,
                vec![NodeId::Imported(absent)],
            ),
        ]),
    );

    assert!(
        !board.frontier.contains_key(&NodeId::Imported(absent)),
        "absent id is not materialised as a node"
    );
    assert!(
        !board.fog.contains_key(&NodeId::Imported(absent)),
        "absent id is not materialised in fog either"
    );
    let fog_b = board
        .fog
        .get(&NodeId::Imported(b))
        .expect("b is fogged behind the absent blocker");
    assert_eq!(fog_b.waits_on, BTreeSet::from([NodeId::Imported(absent)]));
    assert_eq!(
        board.frontier.keys().copied().collect::<BTreeSet<NodeId>>(),
        BTreeSet::from([NodeId::Imported(a)]),
        "only the input set is projected"
    );
}

// ---------------------------------------------------------------------------
// Imported edges can form the cycles §2 promised would arrive via step 4.
// ---------------------------------------------------------------------------

#[test]
fn imported_edges_can_form_cycles_the_frontier_surfaces() {
    let x = ImportedItemId::new();
    let y = ImportedItemId::new();
    let x_item = imported(x, "r1", ImportedItemState::Open, vec![NodeId::Imported(y)]);
    let y_item = imported(y, "r1", ImportedItemState::Open, vec![NodeId::Imported(x)]);
    let board = compute_frontier(&BTreeMap::new(), &[], &imported_map(vec![x_item, y_item]));

    assert_eq!(
        board.cycles.len(),
        1,
        "the cross-imported cycle is named, once"
    );
    let cycle = board.cycles.first().expect("the cycle");
    assert_eq!(cycle.len(), 2);
    assert!(cycle.contains(&NodeId::Imported(x)));
    assert!(cycle.contains(&NodeId::Imported(y)));
    assert!(!board.frontier.contains_key(&NodeId::Imported(x)));
    assert!(!board.frontier.contains_key(&NodeId::Imported(y)));
    assert!(board.fog.contains_key(&NodeId::Imported(x)));
    assert!(board.fog.contains_key(&NodeId::Imported(y)));
}

// ---------------------------------------------------------------------------
// Provenance survives into every imported set — ExternalUnverified, never
// elided, exactly as `property_provenance_survives` asserts MjolnrGoverned.
// ---------------------------------------------------------------------------

#[test]
fn imported_provenance_survives_into_every_set() {
    let f = ImportedItemId::new();
    let s = ImportedItemId::new();
    let w = ImportedItemId::new();
    let board = compute_frontier(
        &BTreeMap::new(),
        &[],
        &imported_map(vec![
            imported(f, "r1", ImportedItemState::Open, Vec::new()),
            imported(s, "r1", ImportedItemState::Merged, Vec::new()),
            imported(w, "r1", ImportedItemState::Open, vec![NodeId::Imported(f)]),
        ]),
    );

    for (id, node) in board
        .frontier
        .iter()
        .chain(board.settled.iter())
        .chain(board.fog.iter().map(|(id, fogged)| (id, &fogged.node)))
    {
        if matches!(id, NodeId::Imported(_)) {
            assert_eq!(node.id, *id);
            assert_eq!(
                node.provenance,
                Provenance::ExternalUnverified,
                "every imported entry is externalUnverified, never elided"
            );
            assert_eq!(node.kind, NodeKind::Implementation);
        }
    }
    assert!(board.frontier.contains_key(&NodeId::Imported(f)));
    assert!(board.settled.contains_key(&NodeId::Imported(s)));
    assert!(board.fog.contains_key(&NodeId::Imported(w)));
}
