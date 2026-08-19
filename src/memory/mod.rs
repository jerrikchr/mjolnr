//! The memory capability module (master implementation plan, Phase 1).
//!
//! A **projection, never authority** (Standing Law #2): everything here —
//! rule snapshots, knowledge triples, recall indexes — is disposable and
//! regenerable. The append-only event ledger and the working tree are truth.
//! Memory may improve context selection; it may never widen policy, approve
//! an action, or become durable truth. Losing `.mjolnr/data/memory.db` is an
//! inconvenience, never data loss.
//!
//! The module is deliberately shaped like `repository`: the runtime calls
//! into it, never the reverse, and it reaches no client, no provider, and no
//! store. The one deliberate import is `policy::paths` — containment has one
//! answer in this codebase, and the rules loader recheck it immediately
//! before every read.
//!
//! Tiers:
//!
//! - **Tier 1** ([`rules`]): diffable Markdown under `.mjolnr/rules/*.md` and
//!   `.mjolnr/USER.md`, loaded once into a frozen snapshot at session start.
//! - **Tier 2** ([`store`]): temporal knowledge triples in SQLite with
//!   automatic `valid_until` invalidation.
//! - **Tier 3** ([`store`]): progressive-disclosure recall — search returns
//!   one-line summaries, timeline returns chronology, expand returns detail
//!   only for named ids.
//! - **Consolidation** ([`consolidation`]): background episodic summarization
//!   and distillation from event ledger into `.mjolnr/data/memory.db`.

pub mod consolidation;
pub mod error;
pub mod rules;
pub mod store;

pub use consolidation::{MAX_EVENTS_PER_PASS, consolidate_events};
pub use error::MemoryError;
pub use rules::{RuleDocument, RulesSnapshot};
pub use store::{Episode, MemoryStore, RecallHit, Triple};
