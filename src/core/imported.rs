//! Imported work items from external trackers (Phase E5, step 4a).
//!
//! A decision ticket settles an unknown; an imported item does work that
//! lives elsewhere. The two kinds have different trust and terminal questions,
//! which is why `design §2` keeps them separate and `frontier::Provenance`
//! survives into every set. Three rules this module enforces, stained once:
//!
//! 1. An imported item is `ExternalUnverified` data. Its title and body arrive
//!    from outside the session — they are never authority (AGENTS.md §11.6).
//! 2. Its state is **observed outcome, never a gate signal**. A remote gate is
//!    not smed's gate (the E5 contract (b)): the frontier
//!    settles an imported item only on a terminal *outcome* it observed,
//!    never on a claim about enforcement.
//! 3. `Unknown` is a real state, never `Open`, and never cached as a value
//!    (the E5 contract (c)). A failed enrichment that yields
//!    `Unknown` does not become `Open` and does not survive as a terminal
//!    value — a re-fetch that supplies a terminal state supersedes it.
//!
//! The serde derives exist for one purpose: these are durable records that
//! `store::wire` will carry in 4b. No client wire depends on them directly — a
//! separate DTO projects them, the same narrow exception `core::review`'s types
//! carry (per `src/store/wire/mod.rs`).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::frontier::NodeId;

/// Identifies one imported item permanently — an external `remote_id` scoped
/// under an `integration` label, minted by the import path when a `RemoteTask`
/// is first fetched. `Uuid::now_v7()` keeps the same ordering property
/// `DecisionTicketId` relies on for the frontier's pure tie-break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ImportedItemId(Uuid);

impl ImportedItemId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for ImportedItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ImportedItemId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// What the remote says about this item, as observed at `fetched_revision`.
///
/// Only *outcomes* — never an enforcement signal. A provider-side gate varies
/// by caller privilege in ways smed cannot verify, so "the forge would refuse
/// it" is not a fact the type can carry (the E5 contract (b)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportedItemState {
    Open,
    Closed,
    Merged,
    Done,
    /// We asked and did not learn — the enrichment read failed and `null` is
    /// not a value to cache. Distinct from `Open`, never terminal (contract (c)).
    Unknown,
}

impl ImportedItemState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Closed | Self::Merged | Self::Done)
    }
}

/// One work item as fetched from an external tracker, projected onto the board.
///
/// A struct rather than a raw `RemoteTask`: the frontier needs a stable board
/// id (`ImportedItemId`) and a blocking graph (`blocked_by`), and the durable
/// path in 4b needs a single record type to replay. Text fields are bounded at
/// the bridge in 4b; `deny_unknown_fields` is already load-bearing here so an
/// extra key cannot ride along from the remote into a durable record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedItem {
    pub id: ImportedItemId,
    pub integration: String,
    pub remote_id: String,
    pub source_url: String,
    pub fetched_revision: String,
    pub title: String,
    pub state: ImportedItemState,
    pub blocked_by: Vec<NodeId>,
}

/// Why a refresh was refused, as the record saw it.
///
/// Typed rather than prose so the caller can choose the reason code without
/// parsing sentences: a stale pin is `WORKSPACE_STALE_REVISION` (the client
/// can re-fetch and try again), while the other refusals are shape bugs the
/// client cannot retry its way out of. The `Display` impl is the human-facing
/// sentence; tests and callers branch on the variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshRefusal {
    /// The refresh names an item this record does not hold.
    UnknownItem {
        named: ImportedItemId,
        held: ImportedItemId,
    },
    /// The revision the human saw when they approved the refresh no longer
    /// matches the record's (contract (a)): a stale tab is refused, not
    /// recorded.
    StaleRevision { expected: String, current: String },
    /// Re-recording the revision the item already carries would hide that the
    /// remote moved.
    SameRevision { revision: String },
    /// `integration` and `remote_id` are immutable identity; a refresh cannot
    /// move them.
    IdentityMoved,
}

impl std::fmt::Display for RefreshRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownItem { named, held } => {
                write!(formatter, "refresh {named} names unknown item {held}")
            }
            Self::StaleRevision { expected, current } => write!(
                formatter,
                "stale revision: expected {expected}, current is {current} — a stale tab is \
                 refused, not recorded"
            ),
            Self::SameRevision { revision } => write!(
                formatter,
                "a refresh must carry a new fetched_revision, not {revision} again; \
                 re-recording the same revision would hide that the remote moved"
            ),
            Self::IdentityMoved => write!(
                formatter,
                "an imported item's integration and remote_id are immutable identity; a \
                 refresh cannot move them"
            ),
        }
    }
}

/// One imported item and its live record, as the runtime holds it.
///
/// Like `DecisionTicketRecord`, this is a working record over the durable log;
/// permanence comes from the events 4b adds. `apply_refresh` enforces contract
/// (a) at the record level: a refresh must name the revision it was rendered
/// for, and a stale tab is refused, not recorded. The session fold delegates
/// here rather than re-checking, so the live path and the replay path apply
/// the same guard (`apply_board_event`'s pattern for resolutions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedItemRecord {
    pub item: ImportedItem,
}

impl ImportedItemRecord {
    #[must_use]
    pub fn new(item: ImportedItem) -> Self {
        Self { item }
    }

    pub fn apply_refresh(
        &mut self,
        expected_revision: &str,
        new_item: ImportedItem,
    ) -> Result<(), RefreshRefusal> {
        if new_item.id != self.item.id {
            return Err(RefreshRefusal::UnknownItem {
                named: new_item.id,
                held: self.item.id,
            });
        }
        if self.item.fetched_revision != expected_revision {
            return Err(RefreshRefusal::StaleRevision {
                expected: expected_revision.to_owned(),
                current: self.item.fetched_revision.clone(),
            });
        }
        if new_item.fetched_revision == expected_revision {
            return Err(RefreshRefusal::SameRevision {
                revision: expected_revision.to_owned(),
            });
        }
        if new_item.integration != self.item.integration
            || new_item.remote_id != self.item.remote_id
        {
            return Err(RefreshRefusal::IdentityMoved);
        }
        self.item = new_item;
        Ok(())
    }
}

/// Why an act over an imported item was refused *before* it left smed.
///
/// The act path's half of contract (a). `RefreshRefusal` guards what enters the
/// record; this guards what leaves for the remote — a post is pinned to the
/// revision the human was looking at when they approved it, and a pin that no
/// longer matches what smed recorded is refused rather than posted against
/// whatever the remote holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActRefusal {
    /// No imported item names this remote, so there is no revision smed
    /// rendered and nothing the pin could honestly refer to. Fail closed: a
    /// post smed cannot tie to something a human saw is not a post it makes.
    NeverImported {
        integration: String,
        remote_id: String,
    },
    /// The pin names a revision the record no longer holds (contract (a)).
    StaleRevision { expected: String, current: String },
}

impl std::fmt::Display for ActRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeverImported {
                integration,
                remote_id,
            } => write!(
                formatter,
                "no imported item names {integration} item {remote_id} in this session, so the \
                 revision pin refers to nothing smed rendered; import it first — nothing was \
                 posted"
            ),
            Self::StaleRevision { expected, current } => write!(
                formatter,
                "stale revision: the change was rendered for {expected} but the item now reads \
                 {current} — a stale tab is refused, not posted; nothing was sent to the remote"
            ),
        }
    }
}

/// Contract (a) on the act path: a mutating act names the revision it was
/// rendered for, and smed refuses it if that is not what it recorded.
///
/// One implementation, called before any network work exists to call it, so the
/// producers that arrive with §D6 inherit the guard rather than each re-deriving
/// it — the drift `apply_refresh` was consolidated to remove.
///
/// When several records name the same remote — two fetches mint two
/// `ImportedItemId`s — the pin must match one of them. It is the *revision*
/// being pinned, not the board row, so any record carrying it is proof the human
/// saw that revision; the refusal names the first mismatch it found.
pub fn check_act_pin<'a>(
    items: impl IntoIterator<Item = &'a ImportedItem>,
    integration: &str,
    remote_id: &str,
    expected_revision: &str,
) -> Result<&'a ImportedItem, ActRefusal> {
    let matching: Vec<&ImportedItem> = items
        .into_iter()
        .filter(|item| item.integration == integration && item.remote_id == remote_id)
        .collect();
    let Some(first) = matching.first() else {
        return Err(ActRefusal::NeverImported {
            integration: integration.to_owned(),
            remote_id: remote_id.to_owned(),
        });
    };
    matching
        .iter()
        .copied()
        .find(|item| item.fetched_revision == expected_revision)
        .ok_or_else(|| ActRefusal::StaleRevision {
            expected: expected_revision.to_owned(),
            current: first.fetched_revision.clone(),
        })
}

/// Intersection containment for imported text (§E5 contract (d)).
///
/// Wherever a model proposes acts over imported text — a label, a work-item
/// id, a comment target — the proposals are intersected with sets computed
/// independently of the model: the real set that exists on the remote and the
/// batch that was actually shown. Injected text can therefore neither invent an
/// object nor reach one that was not shown. The operation is a pure set
/// intersection, never prompt engineering, and it is applied *after* the model
/// responds and *before* any effect.
#[must_use]
pub fn contain_proposals(
    proposed: &[String],
    real_ids: &std::collections::BTreeSet<String>,
    shown_batch: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for candidate in proposed {
        if seen.contains(candidate) {
            continue;
        }
        if real_ids.contains(candidate) && shown_batch.contains(candidate) {
            seen.insert(candidate.clone());
            out.push(candidate.clone());
        }
    }
    out
}

/// The durable identity of one mutating act on an imported item (phase D6,
/// step 6) — a submitted pull request, or the honest `Uncertain` attempt whose
/// result protocol was not proven.
///
/// `Uuid::now_v7()` keeps the same ordering property `ImportedItemId` relies
/// on: acts on one item read newest-first by construction, with no clock
/// stored alongside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ImportedActId(Uuid);

impl ImportedActId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for ImportedActId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ImportedActId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// What a mutating act sent to an external tracker does (phase D6, step 5).
///
/// One kind exists today — a pull request — and it is the seams that matter,
/// not the enum length: a new producer lands a new kind here, and the board
/// history projector already knows how to label it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportedActKind {
    PullRequest,
}

/// The outcome of a submitted act, as smed can honestly state it.
///
/// `Submitted` carries the remote identity (a PR's `html_url`) the provider
/// returned *after* the accepting response — the same evidence boundary as
/// every other imported fact. `Uncertain` is the recovery path: the accepting
/// call went out but its result protocol was not proven, so smed records the
/// attempt without claiming a success it cannot evidence, and recovery
/// governance owns the ambiguity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportedActOutcome {
    Submitted { remote_url: String },
    Uncertain,
}

/// A durable record of a mutating act smed performed on an imported work
/// item (phase D6, step 5) — submitted pull requests and, explicitly, the
/// attempts whose outcome is unknown.
///
/// The record exists to be the history the board projects; it is not a gate
/// and never was. No field here is something a policy guard branches on — the
/// guards that govern an act were already passed by the deterministic code
/// that reached a producer; this record reports what shipped.
/// `expected_revision` is the pin the human approved against, preserved so the
/// history says what was rendered even after the item later refreshes.
///
/// `item_id` names the imported item the act was performed on; mixing the two
/// identities is refused on decode (the exact reverse of mixing them in the
/// `RemoteChangeRequest`). Ordered per item and bounded at projection; the
/// durable source of truth is the event log, so this struct exists only for
/// `store::wire` durability and the session fold — the same narrow exception
/// `ImportedItem` uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedAct {
    /// Durable ordered identity for this act.
    pub act_id: ImportedActId,
    /// The imported item the act was made on.
    pub item_id: ImportedItemId,
    /// What was sent (`PullRequest` — nothing else exists yet).
    pub kind: ImportedActKind,
    /// The revision the human approved against (contract (a) pin).
    pub expected_revision: String,
    /// The head branch the change moved.
    pub head_branch: String,
    /// The base branch the change targeted.
    pub base_branch: String,
    /// What smed can prove happened.
    pub outcome: ImportedActOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_with_revision(revision: &str, state: ImportedItemState) -> ImportedItem {
        ImportedItem {
            id: ImportedItemId::new(),
            integration: "github".to_owned(),
            remote_id: "42".to_owned(),
            source_url: "https://example.invalid/42".to_owned(),
            fetched_revision: revision.to_owned(),
            title: "an imported task".to_owned(),
            state,
            blocked_by: Vec::new(),
        }
    }

    #[test]
    fn an_imported_item_carries_no_field_a_gate_could_branch_on() {
        let item = item_with_revision("rev1", ImportedItemState::Open);
        let value = serde_json::to_value(&item).expect("serialization is total here");
        let object = value.as_object().expect("an imported item is an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "blockedBy",
                "fetchedRevision",
                "id",
                "integration",
                "remoteId",
                "sourceUrl",
                "state",
                "title"
            ],
            "only the eight declared fields may exist: {keys:?}"
        );
        for forbidden in [
            "mergeable",
            "mergeable_state",
            "approved",
            "policy",
            "gate",
            "verdict",
        ] {
            assert!(
                !keys.contains(&forbidden),
                "a gate-shaped field '{forbidden}' must not exist on the type"
            );
        }
    }

    #[test]
    fn unknown_is_a_distinct_state_not_open() {
        assert_ne!(ImportedItemState::Unknown, ImportedItemState::Open);
        assert!(!ImportedItemState::Unknown.is_terminal());
        assert!(!ImportedItemState::Open.is_terminal());
        assert!(ImportedItemState::Closed.is_terminal());
        assert!(ImportedItemState::Merged.is_terminal());
        assert!(ImportedItemState::Done.is_terminal());
    }

    #[test]
    fn a_refresh_pinned_to_the_wrong_revision_is_refused() {
        let first = item_with_revision("rev1", ImportedItemState::Unknown);
        let mut record = ImportedItemRecord::new(first.clone());
        let mut second = first.clone();
        second.fetched_revision = "rev2".to_owned();
        second.state = ImportedItemState::Open;

        let refusal = record
            .apply_refresh("stale-rev", second.clone())
            .expect_err("stale tab must refuse");
        assert_eq!(
            refusal,
            RefreshRefusal::StaleRevision {
                expected: "stale-rev".to_owned(),
                current: "rev1".to_owned()
            }
        );

        record
            .apply_refresh("rev1", second.clone())
            .expect("pinned to the current revision applies");
        assert_eq!(record.item.fetched_revision, "rev2");

        let mut same_rev = second.clone();
        same_rev.state = ImportedItemState::Closed;
        let refusal = record
            .apply_refresh("rev2", same_rev)
            .expect_err("same revision must refuse");
        assert_eq!(
            refusal,
            RefreshRefusal::SameRevision {
                revision: "rev2".to_owned()
            }
        );
    }

    #[test]
    fn a_refresh_that_moves_the_identity_or_names_a_stranger_is_refused() {
        let first = item_with_revision("rev1", ImportedItemState::Open);
        let mut record = ImportedItemRecord::new(first.clone());

        let mut moved = first.clone();
        moved.fetched_revision = "rev2".to_owned();
        moved.remote_id = "43".to_owned();
        assert_eq!(
            record.apply_refresh("rev1", moved),
            Err(RefreshRefusal::IdentityMoved),
            "remote_id is identity; a refresh cannot move it"
        );

        let mut stranger = first.clone();
        stranger.id = ImportedItemId::new();
        stranger.fetched_revision = "rev2".to_owned();
        let named = stranger.id;
        let held = first.id;
        assert_eq!(
            record.apply_refresh("rev1", stranger),
            Err(RefreshRefusal::UnknownItem { named, held }),
            "a refresh cannot create an item; it names one the record must hold"
        );
    }

    // -----------------------------------------------------------------------
    // Contract (a) on the act path
    // -----------------------------------------------------------------------

    #[test]
    fn an_act_pinned_to_the_recorded_revision_is_allowed_through() {
        let item = item_with_revision("rev1", ImportedItemState::Open);
        let allowed = check_act_pin([&item], "github", "42", "rev1").expect("the pin matches");
        assert_eq!(allowed.id, item.id);
    }

    #[test]
    fn an_act_pinned_to_a_revision_the_record_no_longer_holds_is_refused() {
        let item = item_with_revision("rev2", ImportedItemState::Open);
        assert_eq!(
            check_act_pin([&item], "github", "42", "rev1"),
            Err(ActRefusal::StaleRevision {
                expected: "rev1".to_owned(),
                current: "rev2".to_owned(),
            }),
            "the human approved a change against rev1; the item has moved to rev2"
        );
    }

    #[test]
    fn an_act_over_a_remote_that_was_never_imported_is_refused_rather_than_trusted() {
        let item = item_with_revision("rev1", ImportedItemState::Open);
        assert_eq!(
            check_act_pin([&item], "github", "43", "rev1"),
            Err(ActRefusal::NeverImported {
                integration: "github".to_owned(),
                remote_id: "43".to_owned(),
            }),
            "a pin smed cannot tie to a revision it rendered proves nothing"
        );
        assert_eq!(
            check_act_pin([&item], "linear", "42", "rev1"),
            Err(ActRefusal::NeverImported {
                integration: "linear".to_owned(),
                remote_id: "42".to_owned(),
            }),
            "the remote id is scoped by its integration; 'github 42' is not 'linear 42'"
        );
        assert!(
            matches!(
                check_act_pin([], "github", "42", "rev1"),
                Err(ActRefusal::NeverImported { .. })
            ),
            "an empty session refuses; it does not pass an unverifiable pin through"
        );
    }

    /// Two fetches of the same remote mint two board rows. The pin names a
    /// *revision*, so a pin matching either row is a revision the human saw.
    #[test]
    fn a_pin_matching_any_record_of_the_same_remote_is_honoured() {
        let older = item_with_revision("rev1", ImportedItemState::Open);
        let newer = item_with_revision("rev2", ImportedItemState::Open);
        assert!(check_act_pin([&older, &newer], "github", "42", "rev2").is_ok());
        assert!(check_act_pin([&older, &newer], "github", "42", "rev1").is_ok());
        assert_eq!(
            check_act_pin([&older, &newer], "github", "42", "rev3"),
            Err(ActRefusal::StaleRevision {
                expected: "rev3".to_owned(),
                current: "rev1".to_owned(),
            }),
            "a revision no record holds is refused, and the refusal says what is held"
        );
    }

    /// The refusal is a sentence a human acts on, and it states what did *not*
    /// happen — the property AGENTS.md §1.3 asks of every refusal that sits in
    /// front of an outward-facing effect.
    #[test]
    fn an_act_refusal_says_that_nothing_was_posted() {
        let stale = ActRefusal::StaleRevision {
            expected: "rev1".to_owned(),
            current: "rev2".to_owned(),
        }
        .to_string();
        assert!(stale.contains("nothing was sent to the remote"));
        let never = ActRefusal::NeverImported {
            integration: "github".to_owned(),
            remote_id: "42".to_owned(),
        }
        .to_string();
        assert!(never.contains("nothing was posted"));
    }

    #[test]
    fn a_remote_task_refusal_does_not_become_a_value_to_cache() {
        let raw_unknown = r#"{"id":"0199a000-0000-7000-8000-000000000001","integration":"github","remoteId":"42","sourceUrl":"https://example.invalid/42","fetchedRevision":"rev1","title":"t","state":"unknown","blockedBy":[]}"#;
        let parsed: ImportedItem = serde_json::from_str(raw_unknown).expect("unknown parses");
        assert_eq!(parsed.state, ImportedItemState::Unknown);
        assert!(!parsed.state.is_terminal());
    }

    // -----------------------------------------------------------------------
    // Durable act records (phase D6, step 5)
    // -----------------------------------------------------------------------

    /// An act names the item it was made against, and the wire must refuse an
    /// act whose `item_id` does not match its `ImportedItemId` split — the
    /// stored record exists to be provenance, so it cannot mint its own.
    #[test]
    fn an_act_record_names_its_item_and_its_remote_fields_survive_serde() {
        let act = ImportedAct {
            act_id: ImportedActId::new(),
            item_id: ImportedItemId::new(),
            kind: ImportedActKind::PullRequest,
            expected_revision: "2026-08-06T10:00:00Z".to_owned(),
            head_branch: "feat/harness".to_owned(),
            base_branch: "main".to_owned(),
            outcome: ImportedActOutcome::Submitted {
                remote_url: "https://github.com/example/project/pull/7".to_owned(),
            },
        };

        let encoded = serde_json::to_string(&act).expect("act serializes for store::wire");
        let decoded: ImportedAct = serde_json::from_str(&encoded).expect("act round-trips");
        assert_eq!(decoded, act);
        assert_eq!(
            decoded.kind,
            ImportedActKind::PullRequest,
            "one kind today; a producer adds a kind, the board labels it"
        );
        assert_eq!(decoded.act_id, act.act_id);
        assert_eq!(decoded.item_id, act.item_id);
    }

    #[test]
    fn an_uncertain_act_is_a_first_class_outcome_not_a_parsed_myth() {
        let encoded = r#"{
            "actId":"019a0000-0000-7000-8000-000000000001",
            "itemId":"019a0000-0000-7000-8000-000000000002",
            "kind":"pull-request",
            "expectedRevision":"rev1",
            "headBranch":"feat/x",
            "baseBranch":"main",
            "outcome":"uncertain"
        }"#;
        let act: ImportedAct = serde_json::from_str(encoded).expect("uncertain parse");
        assert_eq!(act.outcome, ImportedActOutcome::Uncertain);
        assert_eq!(
            act.kind,
            ImportedActKind::PullRequest,
            "kebab-case serde spelling matches ImportedItemState"
        );
    }

    #[test]
    fn contain_proposals_intersects_both_the_real_set_and_the_shown_batch() {
        use std::collections::BTreeSet;
        let proposed = ["SIM-1", "SIM-2", "SIM-99", "invented"];
        let real: BTreeSet<String> = ["SIM-1", "SIM-2", "SIM-99"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let shown: BTreeSet<String> = ["SIM-1", "SIM-2"].iter().map(|s| (*s).to_owned()).collect();
        let filtered = contain_proposals(
            &proposed.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
            &real,
            &shown,
        );
        assert_eq!(
            filtered,
            vec!["SIM-1", "SIM-2"],
            "injected id outside shown batch cannot be reached; invented id not in real set"
        );
        let empty_batch: BTreeSet<String> = BTreeSet::new();
        assert!(contain_proposals(&["SIM-1".to_owned()], &real, &empty_batch).is_empty());
        let empty_real: BTreeSet<String> = BTreeSet::new();
        assert!(contain_proposals(&["SIM-1".to_owned()], &empty_real, &shown).is_empty());
        let dup = contain_proposals(&["SIM-1".to_owned(), "SIM-1".to_owned()], &real, &shown);
        assert_eq!(dup, vec!["SIM-1"], "deduped");
    }

    #[test]
    fn act_serde_refuses_rogue_fields_and_rogue_state_words() {
        let tracked_stranger = r#"{
            "actId":"019a0000-0000-7000-8000-000000000001",
            "itemId":"019a0000-0000-7000-8000-000000000002",
            "kind":"pull-request",
            "expectedRevision":"rev1",
            "headBranch":"feat/x",
            "baseBranch":"main",
            "outcome":"submitted",
            "remoteUrl":"https://example.invalid/7",
            "unexpected":"rides along"
        }"#;
        assert!(
            serde_json::from_str::<ImportedAct>(tracked_stranger).is_err(),
            "deny_unknown_fields rejects an extra key"
        );
        assert!(
            serde_json::from_str::<ImportedAct>(
                r#"{"actId":"019a0000-0000-7000-8000-000000000001","itemId":"019a0000-0000-7000-8000-000000000002","kind":"pull-request","expectedRevision":"rev1","headBranch":"feat/x","baseBranch":"main","outcome":"gone"}"#
            )
            .is_err(),
            "an outcome word smed never defined is refused, not mapped to Uncertain"
        );
    }
}
