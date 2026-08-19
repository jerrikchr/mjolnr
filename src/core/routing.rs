//! Deterministic route configuration, advance conditions, and the per-provider
//! circuit breaker.
//!
//! Provenance: a 2026-07-20 review of comparable routers — the chain-
//! plus-conditions shape follows 9router, the breaker vocabulary follows
//! wayland-core, and both explicitly reject learned/bandit routing. Nothing
//! here scores "what worked" or recalls history to pick a hop; a route is an
//! ordered list a human wrote down, and an advance is a typed condition firing
//! against that list. If a decision here cannot be predicted from config the
//! user can read, it does not belong in this module.
//!
//! What lives here crosses the `core` boundary the way
//! [`crate::core::trigger`] does: display/event-carrying value types only.
//! File loading (`serde_yaml_ng`, `std::fs`) lives in the top-level `routing`
//! module, exactly as trigger file loading lives in `triggers` rather than
//! `core::trigger`.

use time::OffsetDateTime;

use crate::core::error::ReasonCode;
use crate::core::model::{ModelId, ProviderId};

/// One provider/model pair in an ordered fallback chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteHop {
    pub provider: ProviderId,
    pub model: ModelId,
}

/// Failure threshold and recovery timeout for one provider's circuit breaker.
///
/// Per-provider, not per-route: the plan is explicit ("circuit breaker per
/// provider"). Two routes that both name `anthropic` observe the same breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerConfig {
    pub failure_threshold: u32,
    pub recovery_timeout: std::time::Duration,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: std::time::Duration::from_mins(1),
        }
    }
}

/// A named, ordered fallback chain, optionally tagged with roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDefinition {
    pub name: String,
    pub hops: Vec<RouteHop>,
    /// Role tags this route answers to ( "Named roles").
    ///
    /// A role is an alias *declared on the route it points at*, not an entry
    /// in a parallel table. That is the whole reason roles do not violate the
    /// phase's "no second routing table" anti-pattern: removing every role tag
    /// leaves the routes themselves untouched, and a role that resolves to
    /// nothing falls through to the mechanisms Phase 15 already had.
    pub roles: Vec<String>,
    /// The persona this route wears , by name. A persona is
    /// bound to the *route* — and so to whatever roles alias it — not to a bare
    /// model: the same model earns a different voice depending on the role it
    /// fills. The name resolves to a `.mjolnr/personas/<name>.md` file at prompt
    /// assembly; `None` means the route runs the bare Soul, with no overlay.
    pub persona: Option<String>,
}

impl RouteDefinition {
    #[must_use]
    pub fn hop(&self, position: usize) -> Option<&RouteHop> {
        self.hops.get(position)
    }
}

/// The role names smed gives meaning to out of the box.
///
/// A project may tag a route with any name it likes; these are the ones a
/// tool or spawn can expect to mean something across projects, so they are
/// listed rather than left to convention. Being "well known" grants no
/// special resolution behaviour — an unmapped `smol` and an unmapped
/// `my-project-role` fall back identically.
pub const WELL_KNOWN_ROLES: [&str; 4] = ["default", "smol", "slow", "plan"];

/// Whether a role name is usable as a tag.
///
/// Deliberately narrow, matching the shape route names already have on disk:
/// a role becomes a lookup key and appears in evidence, so it may not be
/// empty, carry whitespace, or hide a leading/trailing space that makes two
/// visually identical configs behave differently.
#[must_use]
pub fn is_valid_role_name(role: &str) -> bool {
    !role.is_empty()
        && role.len() <= 64
        && role.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

/// Where a route selection came from — evidence for "why this model".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteSelectionReason {
    /// The caller named the route explicitly.
    Named,
    /// A requested role resolved to a route tagged with it.
    Role(String),
    /// A role was requested but no route carries that tag, so the route the
    /// caller named literally was used instead.
    ///
    /// This variant exists so the fallback is *stated*: a reader of the event
    /// log can tell "the project maps this role" from "the project does not,
    /// and the caller's own default applied" without inferring it.
    NamedAfterUnmappedRole(String),
    /// Resolved from a task-class mapping.
    TaskClass(String),
    /// A child spawn named no route; the configured child default applied.
    ChildDefault,
}

/// The typed condition that fired to advance (or exhaust) a route.
///
/// Deliberately closed to these three ( "Implement"). No
/// success-rate or latency-scored condition may be added here — that is
/// exactly the learned-routing anti-pattern the phase rejects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteAdvanceCondition {
    /// The Phase 10 quota reserve for the current hop's provider breached its
    /// hard threshold, from existing `QuotaSnapshot`-fed reserve data. Never a
    /// probe request.
    QuotaReserveBreached,
    /// The current hop's provider returned a typed failure.
    ProviderFailure(ReasonCode),
    /// The current hop's provider circuit breaker is open.
    BreakerOpen,
}

impl RouteAdvanceCondition {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::QuotaReserveBreached => "quota reserve breached",
            Self::ProviderFailure(_) => "typed provider failure",
            Self::BreakerOpen => "circuit breaker open",
        }
    }
}

/// What advancing one position along a route came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvanceOutcome {
    Advanced {
        position: usize,
        hop: RouteHop,
    },
    /// No viable position remains. A typed stop, never a silent retry loop.
    Exhausted,
}

/// Move one position forward along `route`, or report exhaustion.
///
/// Pure and total: the caller supplies the current position, this returns the
/// next fact. It does not consult breaker or quota state — the caller decides
/// *whether* to advance; this only answers *to where*.
#[must_use]
pub fn advance_position(route: &RouteDefinition, position: usize) -> AdvanceOutcome {
    let next = position + 1;
    match route.hop(next) {
        Some(hop) => AdvanceOutcome::Advanced {
            position: next,
            hop: hop.clone(),
        },
        None => AdvanceOutcome::Exhausted,
    }
}

/// A session's live position on an attached route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRuntime {
    pub route: String,
    pub position: usize,
}

/// Circuit breaker lifecycle ("Closed → Open → `HalfOpen`").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

impl BreakerState {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half-open",
        }
    }
}

/// A state transition, evidence for a durable `BreakerStateChanged` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerTransition {
    pub from: BreakerState,
    pub to: BreakerState,
}

/// One provider's circuit breaker.
///
/// Deterministic and clock-driven, not learned: it counts consecutive
/// failures against a configured threshold and reopens for a probe after a
/// configured timeout. Nothing here scores success rate over time or remembers
/// "what worked" — that is the rejected shape.
#[derive(Debug, Clone, PartialEq)]
pub struct CircuitBreaker {
    state: BreakerState,
    consecutive_failures: u32,
    opened_at: Option<OffsetDateTime>,
    config: BreakerConfig,
}

impl CircuitBreaker {
    #[must_use]
    pub const fn new(config: BreakerConfig) -> Self {
        Self {
            state: BreakerState::Closed,
            consecutive_failures: 0,
            opened_at: None,
            config,
        }
    }

    #[must_use]
    pub const fn state(&self) -> BreakerState {
        self.state
    }

    #[must_use]
    pub const fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Reconcile `Open` against the clock before a request is attempted. An
    /// `Open` breaker whose recovery timeout has elapsed becomes `HalfOpen` —
    /// exactly one probe is then permitted by
    /// [`permits_request`](Self::permits_request).
    pub fn poll(&mut self, now: OffsetDateTime) -> Option<BreakerTransition> {
        if self.state != BreakerState::Open {
            return None;
        }
        let opened_at = self.opened_at?;
        if now < opened_at + self.config.recovery_timeout {
            return None;
        }
        let from = self.state;
        self.state = BreakerState::HalfOpen;
        Some(BreakerTransition {
            from,
            to: self.state,
        })
    }

    /// Whether a request may be attempted right now. Call [`poll`](Self::poll)
    /// first so an elapsed recovery timeout is reflected.
    #[must_use]
    pub const fn permits_request(&self) -> bool {
        matches!(self.state, BreakerState::Closed | BreakerState::HalfOpen)
    }

    pub fn on_failure(&mut self, now: OffsetDateTime) -> Option<BreakerTransition> {
        match self.state {
            BreakerState::Closed => {
                self.consecutive_failures += 1;
                if self.consecutive_failures < self.config.failure_threshold {
                    return None;
                }
                let from = self.state;
                self.state = BreakerState::Open;
                self.opened_at = Some(now);
                Some(BreakerTransition {
                    from,
                    to: self.state,
                })
            }
            BreakerState::HalfOpen => {
                let from = self.state;
                self.state = BreakerState::Open;
                self.opened_at = Some(now);
                self.consecutive_failures = self.config.failure_threshold;
                Some(BreakerTransition {
                    from,
                    to: self.state,
                })
            }
            BreakerState::Open => None,
        }
    }

    pub fn on_success(&mut self, now: OffsetDateTime) -> Option<BreakerTransition> {
        let _ = now;
        match self.state {
            BreakerState::Closed => {
                self.consecutive_failures = 0;
                None
            }
            BreakerState::HalfOpen => {
                let from = self.state;
                self.state = BreakerState::Closed;
                self.consecutive_failures = 0;
                self.opened_at = None;
                Some(BreakerTransition {
                    from,
                    to: self.state,
                })
            }
            BreakerState::Open => None,
        }
    }
}

/// One provider's live breaker state, for the `/usage` overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakerView {
    pub provider: ProviderId,
    pub state: BreakerState,
    pub consecutive_failures: u32,
}

/// The resolved routing table for one project: named routes, the derived
/// role index, task-class mapping, the child-spawn default, and per-provider
/// breaker configuration.
#[derive(Debug, Clone, Default)]
pub struct RouteTable {
    pub routes: std::collections::BTreeMap<String, RouteDefinition>,
    /// Role name to route name, derived from the routes' own `roles` tags at
    /// load time. Never written independently of `routes`: it is an index,
    /// and an index that can disagree with what it indexes is a second table
    /// wearing a disguise.
    pub roles: std::collections::BTreeMap<String, String>,
    pub task_classes: std::collections::BTreeMap<String, String>,
    pub child_default: Option<String>,
    pub breakers: std::collections::BTreeMap<String, BreakerConfig>,
}

impl RouteTable {
    /// True when no routing config exists. The caller must fall back to
    /// present-day behaviour exactly: configured provider, no chains, no
    /// breaker ( checklist).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// Rebuild [`roles`](Self::roles) from the routes' own tags.
    ///
    /// Returns the tags that were refused, as `(route name, role, why)`, for
    /// the caller to surface as load diagnostics. Two routes claiming one role
    /// is a real ambiguity, so the first in name order wins and the second is
    /// reported — the same "one bad entry does not block the rest" posture the
    /// route loader already takes, and never a silent last-write-wins.
    pub fn reindex_roles(&mut self) -> Vec<(String, String, String)> {
        let mut index = std::collections::BTreeMap::new();
        let mut refused = Vec::new();
        for (route_name, definition) in &self.routes {
            for role in &definition.roles {
                if !is_valid_role_name(role) {
                    refused.push((
                        route_name.clone(),
                        role.clone(),
                        "a role name must be 1-64 characters of ASCII letters, digits, '-', or '_'"
                            .to_owned(),
                    ));
                    continue;
                }
                match index.entry(role.clone()) {
                    std::collections::btree_map::Entry::Vacant(slot) => {
                        slot.insert(route_name.clone());
                    }
                    std::collections::btree_map::Entry::Occupied(taken) => {
                        refused.push((
                            route_name.clone(),
                            role.clone(),
                            format!("role is already claimed by route '{}'", taken.get()),
                        ));
                    }
                }
            }
        }
        self.roles = index;
        refused
    }

    /// The route a role points at, if any route carries that tag.
    #[must_use]
    pub fn route_for_role(&self, role: &str) -> Option<&RouteDefinition> {
        let name = self.roles.get(role)?;
        self.routes.get(name)
    }

    /// Resolve a route: requested role, else explicit name, else task class.
    ///
    /// Role is tried before the literal name on purpose. A role is the
    /// project's own indirection — "whatever this project considers `smol`" —
    /// while a literal name is the caller's hardcoded choice. When the project
    /// has an opinion, the project wins; when it does not, the caller's name
    /// applies and the reason says so. Reversing this would make a role
    /// unreachable whenever a caller also carried a default, which is every
    /// caller.
    #[must_use]
    pub fn resolve(
        &self,
        requested: Option<&str>,
        role: Option<&str>,
        task_class: &str,
    ) -> Option<(&RouteDefinition, RouteSelectionReason)> {
        if let Some(role) = role {
            if let Some(route) = self.route_for_role(role) {
                return Some((route, RouteSelectionReason::Role(role.to_owned())));
            }
            if let Some(name) = requested {
                return self.routes.get(name).map(|route| {
                    (
                        route,
                        RouteSelectionReason::NamedAfterUnmappedRole(role.to_owned()),
                    )
                });
            }
        }
        if let Some(name) = requested {
            return self
                .routes
                .get(name)
                .map(|route| (route, RouteSelectionReason::Named));
        }
        let name = self.task_classes.get(task_class)?;
        self.routes.get(name).map(|route| {
            (
                route,
                RouteSelectionReason::TaskClass(task_class.to_owned()),
            )
        })
    }

    /// Resolve a child spawn's route: requested role, else explicit name,
    /// else the configured child default. A child spawn that resolves to
    /// nothing keeps the parent's provider exactly as before Phase 15 —
    /// including when it asked for a role this project does not map.
    #[must_use]
    pub fn resolve_child(
        &self,
        requested: Option<&str>,
        role: Option<&str>,
    ) -> Option<(&RouteDefinition, RouteSelectionReason)> {
        if let Some(role) = role {
            if let Some(route) = self.route_for_role(role) {
                return Some((route, RouteSelectionReason::Role(role.to_owned())));
            }
            if let Some(name) = requested {
                return self.routes.get(name).map(|route| {
                    (
                        route,
                        RouteSelectionReason::NamedAfterUnmappedRole(role.to_owned()),
                    )
                });
            }
        }
        if let Some(name) = requested {
            return self
                .routes
                .get(name)
                .map(|route| (route, RouteSelectionReason::Named));
        }
        let name = self.child_default.as_deref()?;
        self.routes
            .get(name)
            .map(|route| (route, RouteSelectionReason::ChildDefault))
    }

    #[must_use]
    pub fn breaker_config(&self, provider: &ProviderId) -> BreakerConfig {
        self.breakers
            .get(provider.as_str())
            .copied()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(name: &str, hops: &[(&str, &str)]) -> RouteDefinition {
        tagged_route(name, hops, &[])
    }

    fn tagged_route(name: &str, hops: &[(&str, &str)], roles: &[&str]) -> RouteDefinition {
        RouteDefinition {
            name: name.to_owned(),
            hops: hops
                .iter()
                .map(|(provider, model)| RouteHop {
                    provider: ProviderId::new(*provider),
                    model: ModelId::new(*model),
                })
                .collect(),
            roles: roles.iter().map(|role| (*role).to_owned()).collect(),
            persona: None,
        }
    }

    #[test]
    fn advance_walks_the_chain_and_then_exhausts() {
        let definition = route("main", &[("a", "m1"), ("b", "m2")]);
        match advance_position(&definition, 0) {
            AdvanceOutcome::Advanced { position, hop } => {
                assert_eq!(position, 1);
                assert_eq!(hop.provider, ProviderId::new("b"));
            }
            AdvanceOutcome::Exhausted => panic!("expected an advance"),
        }
        assert_eq!(advance_position(&definition, 1), AdvanceOutcome::Exhausted);
    }

    #[test]
    fn resolve_prefers_an_explicit_name_over_task_class() {
        let mut table = RouteTable::default();
        table
            .routes
            .insert("main".to_owned(), route("main", &[("a", "m1")]));
        table
            .routes
            .insert("cheap".to_owned(), route("cheap", &[("b", "m2")]));
        table
            .task_classes
            .insert("default".to_owned(), "main".to_owned());

        let (resolved, reason) = table
            .resolve(Some("cheap"), None, "default")
            .expect("resolved");
        assert_eq!(resolved.name, "cheap");
        assert_eq!(reason, RouteSelectionReason::Named);

        let (resolved, reason) = table.resolve(None, None, "default").expect("resolved");
        assert_eq!(resolved.name, "main");
        assert_eq!(
            reason,
            RouteSelectionReason::TaskClass("default".to_owned())
        );

        assert!(table.resolve(None, None, "unmapped").is_none());
    }

    #[test]
    fn resolve_child_falls_back_to_the_configured_default() {
        let mut table = RouteTable::default();
        table
            .routes
            .insert("cheap".to_owned(), route("cheap", &[("b", "m2")]));
        table.child_default = Some("cheap".to_owned());

        let (resolved, reason) = table.resolve_child(None, None).expect("resolved");
        assert_eq!(resolved.name, "cheap");
        assert_eq!(reason, RouteSelectionReason::ChildDefault);

        assert!(table.resolve_child(Some("missing"), None).is_none());
    }

    #[test]
    fn a_default_table_has_no_routes_and_no_breakers() {
        let table = RouteTable::default();
        assert!(table.is_empty());
        assert!(table.resolve(None, None, "default").is_none());
        assert!(table.resolve_child(None, None).is_none());
    }

    fn role_table() -> RouteTable {
        let mut table = RouteTable::default();
        table.routes.insert(
            "main".to_owned(),
            tagged_route("main", &[("a", "m1")], &["default"]),
        );
        table.routes.insert(
            "cheap".to_owned(),
            tagged_route("cheap", &[("b", "m2")], &["smol"]),
        );
        let refused = table.reindex_roles();
        assert!(refused.is_empty(), "fixture roles must all be accepted");
        table
    }

    #[test]
    fn a_role_resolves_to_the_route_tagged_with_it() {
        let table = role_table();
        let (resolved, reason) = table
            .resolve(None, Some("smol"), "default")
            .expect("resolved");
        assert_eq!(resolved.name, "cheap");
        assert_eq!(reason, RouteSelectionReason::Role("smol".to_owned()));
    }

    #[test]
    fn a_role_resolves_to_the_same_position_a_direct_name_would() {
        // The phase's checklist: a role is sugar over a route, so it must land
        // on exactly what naming that route directly lands on.
        let table = role_table();
        let (by_role, _) = table
            .resolve(None, Some("smol"), "default")
            .expect("by role");
        let (by_name, _) = table
            .resolve(Some("cheap"), None, "default")
            .expect("by name");
        assert_eq!(by_role, by_name);
        assert_eq!(by_role.hop(0), by_name.hop(0));
    }

    #[test]
    fn an_unmapped_role_falls_back_to_the_literal_name_and_says_so() {
        let table = role_table();
        let (resolved, reason) = table
            .resolve(Some("main"), Some("absent"), "default")
            .expect("resolved");
        assert_eq!(resolved.name, "main");
        assert_eq!(
            reason,
            RouteSelectionReason::NamedAfterUnmappedRole("absent".to_owned())
        );
    }

    #[test]
    fn an_unmapped_role_with_no_literal_name_falls_through_to_task_class() {
        let mut table = role_table();
        table
            .task_classes
            .insert("default".to_owned(), "main".to_owned());
        let (resolved, reason) = table
            .resolve(None, Some("absent"), "default")
            .expect("resolved");
        assert_eq!(resolved.name, "main");
        assert_eq!(
            reason,
            RouteSelectionReason::TaskClass("default".to_owned())
        );
    }

    #[test]
    fn a_role_beats_a_literal_name_because_the_project_owns_the_mapping() {
        let table = role_table();
        let (resolved, reason) = table
            .resolve(Some("main"), Some("smol"), "default")
            .expect("resolved");
        assert_eq!(resolved.name, "cheap");
        assert_eq!(reason, RouteSelectionReason::Role("smol".to_owned()));
    }

    #[test]
    fn a_child_spawn_resolves_a_role_and_falls_back_when_it_is_unmapped() {
        let mut table = role_table();
        table.child_default = Some("main".to_owned());

        let (resolved, reason) = table.resolve_child(None, Some("smol")).expect("resolved");
        assert_eq!(resolved.name, "cheap");
        assert_eq!(reason, RouteSelectionReason::Role("smol".to_owned()));

        let (resolved, reason) = table.resolve_child(None, Some("absent")).expect("resolved");
        assert_eq!(resolved.name, "main");
        assert_eq!(reason, RouteSelectionReason::ChildDefault);
    }

    #[test]
    fn two_routes_claiming_one_role_refuse_the_second_in_name_order() {
        let mut table = RouteTable::default();
        table.routes.insert(
            "alpha".to_owned(),
            tagged_route("alpha", &[("a", "m1")], &["smol"]),
        );
        table.routes.insert(
            "beta".to_owned(),
            tagged_route("beta", &[("b", "m2")], &["smol"]),
        );

        let refused = table.reindex_roles();
        assert_eq!(refused.len(), 1);
        let (route, role, detail) = refused.first().expect("one refusal");
        assert_eq!(route, "beta");
        assert_eq!(role, "smol");
        assert!(detail.contains("alpha"));
        assert_eq!(table.roles.get("smol"), Some(&"alpha".to_owned()));
    }

    #[test]
    fn an_invalid_role_name_is_refused_and_never_indexed() {
        let mut table = RouteTable::default();
        table.routes.insert(
            "main".to_owned(),
            tagged_route("main", &[("a", "m1")], &["not a role"]),
        );
        let refused = table.reindex_roles();
        assert_eq!(refused.len(), 1);
        assert!(table.roles.is_empty());
        assert!(table.route_for_role("not a role").is_none());
    }

    #[test]
    fn well_known_role_names_are_all_valid_names() {
        assert!(WELL_KNOWN_ROLES.iter().all(|role| is_valid_role_name(role)));
        assert!(!is_valid_role_name(""));
        assert!(!is_valid_role_name(" smol"));
        assert!(!is_valid_role_name(&"r".repeat(65)));
    }

    fn epoch(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds).expect("valid unix timestamp")
    }

    #[test]
    fn breaker_opens_after_the_configured_consecutive_failures() {
        let config = BreakerConfig {
            failure_threshold: 3,
            recovery_timeout: std::time::Duration::from_secs(30),
        };
        let mut breaker = CircuitBreaker::new(config);
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert!(breaker.on_failure(epoch(0)).is_none());
        assert!(breaker.on_failure(epoch(1)).is_none());
        let transition = breaker.on_failure(epoch(2)).expect("opens on the third");
        assert_eq!(transition.from, BreakerState::Closed);
        assert_eq!(transition.to, BreakerState::Open);
        assert!(!breaker.permits_request());
    }

    #[test]
    fn a_success_before_threshold_resets_the_failure_count() {
        let config = BreakerConfig {
            failure_threshold: 2,
            recovery_timeout: std::time::Duration::from_secs(30),
        };
        let mut breaker = CircuitBreaker::new(config);
        assert!(breaker.on_failure(epoch(0)).is_none());
        assert!(breaker.on_success(epoch(1)).is_none());
        // Two more failures are needed again, not one, because success reset
        // the counter rather than merely delaying the trip.
        assert!(breaker.on_failure(epoch(2)).is_none());
        assert_eq!(breaker.state(), BreakerState::Closed);
    }

    #[test]
    fn half_open_probes_after_the_recovery_timeout_and_closes_on_success() {
        let config = BreakerConfig {
            failure_threshold: 1,
            recovery_timeout: std::time::Duration::from_mins(1),
        };
        let mut breaker = CircuitBreaker::new(config);
        breaker.on_failure(epoch(0)).expect("opens immediately");
        assert!(!breaker.permits_request());

        // Before the timeout, polling changes nothing.
        assert!(breaker.poll(epoch(30)).is_none());
        assert!(!breaker.permits_request());

        // At/after the timeout, exactly one probe is permitted.
        let transition = breaker.poll(epoch(60)).expect("half-opens at the timeout");
        assert_eq!(transition.from, BreakerState::Open);
        assert_eq!(transition.to, BreakerState::HalfOpen);
        assert!(breaker.permits_request());

        let closed = breaker
            .on_success(epoch(61))
            .expect("closes on a good probe");
        assert_eq!(closed.from, BreakerState::HalfOpen);
        assert_eq!(closed.to, BreakerState::Closed);
        assert!(breaker.permits_request());
    }

    #[test]
    fn a_failed_probe_reopens_the_breaker() {
        let config = BreakerConfig {
            failure_threshold: 1,
            recovery_timeout: std::time::Duration::from_secs(10),
        };
        let mut breaker = CircuitBreaker::new(config);
        breaker.on_failure(epoch(0)).expect("opens");
        breaker.poll(epoch(10)).expect("half-opens");
        let reopened = breaker
            .on_failure(epoch(11))
            .expect("reopens on a bad probe");
        assert_eq!(reopened.from, BreakerState::HalfOpen);
        assert_eq!(reopened.to, BreakerState::Open);
        assert!(!breaker.permits_request());
    }
}
