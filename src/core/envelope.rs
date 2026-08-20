//! The spawn envelope: pre-authorising a bounded population of children
//! .
//!
//! `MAX_CHILDREN = 4` is not an oversight — it is the last link in the chain
//! that holds up mjolnr's central claim:
//!
//! > every spawn is approved individually → the preview must be reviewable →
//! > the preview must be short → the children must be few
//!
//! Raising that cap on its own breaks the chain silently: the preview grows past
//! what anyone reads, the approval becomes a reflex, and "a child never gets
//! authority a human did not grant" becomes technically true and practically
//! meaningless. Width therefore needs a different authorisation primitive rather
//! than a bigger number.
//!
//! An envelope is that primitive: a human approves a *shape* once, and the
//! runtime enforces it per spawn with every draw recorded against it. It is a
//! generalisation of machinery mjolnr already has — `a` (approve this exact
//! command for the session) is the same idea at N=1, and shares the same three
//! properties: bounded, session-scoped, and never durable.

use crate::core::policy::PolicyMode;

/// Ceilings on what a human may arm. Bounds on the bounds: an envelope is a
/// working session's tool, not an overnight fleet, and that distinction is
/// worth keeping.
pub const MAX_ENVELOPE_CHILDREN: u32 = 64;
pub const MAX_ENVELOPE_PER_CALL: u32 = 16;
pub const MAX_ENVELOPE_TURNS: u32 = 200;

/// A shape a human authorised, before any of the spawns that fill it exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnEnvelope {
    /// The widest policy any child under this envelope may run at.
    ///
    /// Clamped *in addition* to [`PolicyMode`]'s ordinary parent clamp, never
    /// instead of it. Arming a ceiling wider than the session's own policy is
    /// refused rather than silently narrowed — a human who asks for something
    /// that cannot be granted should be told, not quietly given less.
    pub ceiling: PolicyMode,
    /// Total children this envelope may authorise over its whole life.
    pub max_children: u32,
    /// Most children one `spawn_subagent` call may draw.
    ///
    /// This is what keeps a single preview readable even under an envelope, so
    /// the property the cap protected survives the cap being lifted.
    pub max_per_call: u32,
    /// Aggregate provider turns across every child drawn. The spend bound.
    pub max_provider_turns: u32,
    /// Roles or routes children may resolve to. Empty means "any the project
    /// configures", which is the default rather than a widening: routing already
    /// decides what a role points at.
    pub routes: Vec<String>,
    /// Turns after which the envelope lapses.
    pub expires_after_turns: u32,
}

/// Turns budgeted per child when an envelope's bounds are derived rather than
/// spelled out.
///
/// Mirrors `tools::subagent::DEFAULT_CHILD_TURNS`, which `core` may not import
/// (`AGENTS.md` §2.1). A test in `tools::subagent` asserts the two agree, so the
/// duplication cannot drift silently.
pub const DEFAULT_TURNS_PER_CHILD: u32 = 8;

impl SpawnEnvelope {
    /// Derive a whole envelope from the one number a human actually reasons
    /// about.
    ///
    /// Per-call width and aggregate spend follow from the child count rather
    /// than being asked for separately: four fields on one line is a form, and a
    /// form is where someone stops reading what they are agreeing to.
    #[must_use]
    pub fn for_children(children: u32, ceiling: PolicyMode, expires_after_turns: u32) -> Self {
        Self {
            ceiling,
            max_children: children,
            max_per_call: children.min(MAX_ENVELOPE_PER_CALL),
            max_provider_turns: children
                .saturating_mul(DEFAULT_TURNS_PER_CHILD)
                .min(MAX_ENVELOPE_TURNS),
            routes: Vec::new(),
            expires_after_turns,
        }
    }
}

/// Why an envelope could not be armed, or a draw could not be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeRefusal {
    /// The ceiling is wider than the session's current policy.
    CeilingExceedsPolicy {
        ceiling: PolicyMode,
        policy: PolicyMode,
    },
    /// A bound is above what any envelope may declare.
    BoundTooLarge { field: &'static str, limit: u32 },
    /// A bound is zero, which would arm an envelope that authorises nothing.
    BoundIsZero { field: &'static str },
    /// This call asked for more children than one call may draw.
    PerCallExceeded { asked: u32, allowed: u32 },
    /// This call asked for more children than the envelope has left.
    Exhausted { asked: u32, remaining: u32 },
    /// A child named a route the envelope does not cover.
    RouteNotCovered { route: String },
    /// The aggregate turn budget cannot fund this draw.
    TurnsExhausted { asked: u32, remaining: u32 },
}

impl EnvelopeRefusal {
    /// A sentence for the human and the model. Says what was asked and what was
    /// available, because "refused" without a number is a refusal nobody can
    /// re-plan against.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::CeilingExceedsPolicy { ceiling, policy } => format!(
                "an envelope ceiling of `{}` is wider than this session's `{}` policy",
                ceiling.label(),
                policy.label()
            ),
            Self::BoundTooLarge { field, limit } => {
                format!("{field} is above the maximum of {limit}")
            }
            Self::BoundIsZero { field } => {
                format!("{field} is zero, which would authorise nothing")
            }
            Self::PerCallExceeded { asked, allowed } => {
                format!("this spawn asked for {asked} children; one call may draw {allowed}")
            }
            Self::Exhausted { asked, remaining } => {
                format!("this spawn asked for {asked} children; {remaining} remain in the envelope")
            }
            Self::RouteNotCovered { route } => {
                format!("the envelope does not cover route `{route}`")
            }
            Self::TurnsExhausted { asked, remaining } => format!(
                "this spawn needs {asked} provider turns; {remaining} remain in the envelope"
            ),
        }
    }
}

impl SpawnEnvelope {
    /// Check an envelope against the policy it would be armed under.
    ///
    /// # Errors
    /// [`EnvelopeRefusal`] when the ceiling is wider than the session's policy,
    /// or a bound is zero or above its maximum.
    pub fn validate(&self, policy: PolicyMode) -> Result<(), EnvelopeRefusal> {
        // Property 2: an envelope never widens authority. A ceiling the session
        // itself does not hold is refused rather than narrowed, so nobody arms
        // one thing and gets another.
        if clamp_ceiling(policy, self.ceiling) != self.ceiling {
            return Err(EnvelopeRefusal::CeilingExceedsPolicy {
                ceiling: self.ceiling,
                policy,
            });
        }
        for (field, value, limit) in [
            ("max_children", self.max_children, MAX_ENVELOPE_CHILDREN),
            ("max_per_call", self.max_per_call, MAX_ENVELOPE_PER_CALL),
            (
                "max_provider_turns",
                self.max_provider_turns,
                MAX_ENVELOPE_TURNS,
            ),
            (
                "expires_after_turns",
                self.expires_after_turns,
                MAX_ENVELOPE_TURNS,
            ),
        ] {
            if value == 0 {
                return Err(EnvelopeRefusal::BoundIsZero { field });
            }
            if value > limit {
                return Err(EnvelopeRefusal::BoundTooLarge { field, limit });
            }
        }
        Ok(())
    }
}

/// The widest policy an envelope may declare under `policy`.
///
/// Deliberately the same shape as `runtime::subagent::clamp_policy`, and applied
/// *as well as* it rather than instead: an envelope narrows, it never grants.
#[must_use]
pub const fn clamp_ceiling(policy: PolicyMode, ceiling: PolicyMode) -> PolicyMode {
    match (policy, ceiling) {
        (PolicyMode::ReadOnly, _) | (_, PolicyMode::ReadOnly) => PolicyMode::ReadOnly,
        (PolicyMode::FullAuto, PolicyMode::FullAuto) => PolicyMode::FullAuto,
        _ => PolicyMode::WorkspaceWrite,
    }
}

/// An armed envelope and what is left of it.
///
/// Never checkpointed and never rebuilt on resume, exactly like
/// `SessionState::exact_commands`. Property 3: unattended autonomy is a thing a
/// human turns on for a stretch of work they are watching, and a session that
/// comes back without them doing anything is not that stretch of work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveEnvelope {
    pub envelope: SpawnEnvelope,
    pub children_drawn: u32,
    pub turns_drawn: u32,
    pub turns_elapsed: u32,
}

impl ActiveEnvelope {
    #[must_use]
    pub const fn new(envelope: SpawnEnvelope) -> Self {
        Self {
            envelope,
            children_drawn: 0,
            turns_drawn: 0,
            turns_elapsed: 0,
        }
    }

    #[must_use]
    pub const fn children_remaining(&self) -> u32 {
        self.envelope
            .max_children
            .saturating_sub(self.children_drawn)
    }

    #[must_use]
    pub const fn turns_remaining(&self) -> u32 {
        self.envelope
            .max_provider_turns
            .saturating_sub(self.turns_drawn)
    }

    /// Whether the envelope has lapsed. Checked before a draw, so an expired
    /// envelope never authorises anything.
    #[must_use]
    pub const fn expired(&self) -> bool {
        self.turns_elapsed >= self.envelope.expires_after_turns
    }

    /// Whether nothing is left to draw. An exhausted envelope is cleared rather
    /// than kept as a zero — a visible "0 remaining" invites the reading that it
    /// is still in force.
    #[must_use]
    pub const fn exhausted(&self) -> bool {
        self.children_remaining() == 0 || self.turns_remaining() == 0
    }

    /// Check a draw of `children` costing `turns`, and the routes they name.
    ///
    /// # Errors
    /// [`EnvelopeRefusal`] when the draw does not fit. Refused rather than
    /// downgraded to an approval prompt: property 4, because a 24-child preview
    /// is the previewability problem coming back through the door it was shown
    /// out of.
    pub fn check_draw(
        &self,
        children: u32,
        turns: u32,
        routes: &[String],
    ) -> Result<(), EnvelopeRefusal> {
        if children > self.envelope.max_per_call {
            return Err(EnvelopeRefusal::PerCallExceeded {
                asked: children,
                allowed: self.envelope.max_per_call,
            });
        }
        if children > self.children_remaining() {
            return Err(EnvelopeRefusal::Exhausted {
                asked: children,
                remaining: self.children_remaining(),
            });
        }
        if turns > self.turns_remaining() {
            return Err(EnvelopeRefusal::TurnsExhausted {
                asked: turns,
                remaining: self.turns_remaining(),
            });
        }
        if !self.envelope.routes.is_empty() {
            for route in routes {
                if !self.envelope.routes.contains(route) {
                    return Err(EnvelopeRefusal::RouteNotCovered {
                        route: route.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Record a draw that [`Self::check_draw`] accepted.
    pub const fn draw(&mut self, children: u32, turns: u32) {
        self.children_drawn = self.children_drawn.saturating_add(children);
        self.turns_drawn = self.turns_drawn.saturating_add(turns);
    }

    /// One turn passed. Returns whether the envelope lapsed on this turn.
    pub const fn tick(&mut self) -> bool {
        self.turns_elapsed = self.turns_elapsed.saturating_add(1);
        self.expired()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope() -> SpawnEnvelope {
        SpawnEnvelope {
            ceiling: PolicyMode::ReadOnly,
            max_children: 32,
            max_per_call: 8,
            max_provider_turns: 100,
            routes: Vec::new(),
            expires_after_turns: 20,
        }
    }

    #[test]
    fn a_ceiling_wider_than_the_session_is_refused_not_narrowed() {
        // The whole point of property 2. Narrowing silently would arm one thing
        // and grant another, which is the class of surprise the gate exists to
        // remove.
        let wide = SpawnEnvelope {
            ceiling: PolicyMode::FullAuto,
            ..envelope()
        };
        assert_eq!(
            wide.validate(PolicyMode::Ask),
            Err(EnvelopeRefusal::CeilingExceedsPolicy {
                ceiling: PolicyMode::FullAuto,
                policy: PolicyMode::Ask,
            })
        );
    }

    #[test]
    fn a_read_only_session_can_only_arm_a_read_only_envelope() {
        for ceiling in [
            PolicyMode::WorkspaceWrite,
            PolicyMode::FullAuto,
            PolicyMode::Ask,
        ] {
            let attempt = SpawnEnvelope {
                ceiling,
                ..envelope()
            };
            assert!(
                attempt.validate(PolicyMode::ReadOnly).is_err(),
                "a read-only session must not arm a `{}` ceiling",
                ceiling.label()
            );
        }
        assert!(envelope().validate(PolicyMode::ReadOnly).is_ok());
    }

    #[test]
    fn zero_and_oversized_bounds_are_both_refused() {
        let zero = SpawnEnvelope {
            max_children: 0,
            ..envelope()
        };
        assert_eq!(
            zero.validate(PolicyMode::ReadOnly),
            Err(EnvelopeRefusal::BoundIsZero {
                field: "max_children"
            })
        );
        let huge = SpawnEnvelope {
            max_children: MAX_ENVELOPE_CHILDREN + 1,
            ..envelope()
        };
        assert_eq!(
            huge.validate(PolicyMode::ReadOnly),
            Err(EnvelopeRefusal::BoundTooLarge {
                field: "max_children",
                limit: MAX_ENVELOPE_CHILDREN,
            })
        );
    }

    #[test]
    fn a_draw_larger_than_the_remainder_is_refused_with_both_numbers() {
        // The scenario from the scope doc: A draws 8, B draws 4, C asks for 24
        // with 20 left and is refused — with enough detail to re-plan against.
        let mut active = ActiveEnvelope::new(envelope());
        assert!(active.check_draw(8, 10, &[]).is_ok());
        active.draw(8, 10);
        assert!(active.check_draw(4, 5, &[]).is_ok());
        active.draw(4, 5);
        assert_eq!(active.children_remaining(), 20);

        assert_eq!(
            active.check_draw(24, 10, &[]),
            Err(EnvelopeRefusal::PerCallExceeded {
                asked: 24,
                allowed: 8
            }),
            "the per-call bound bites first, and it is the one that keeps the preview readable"
        );
    }

    #[test]
    fn the_remainder_bound_bites_when_the_per_call_bound_does_not() {
        let mut active = ActiveEnvelope::new(SpawnEnvelope {
            max_children: 10,
            max_per_call: 8,
            ..envelope()
        });
        active.draw(6, 10);
        assert_eq!(
            active.check_draw(8, 5, &[]),
            Err(EnvelopeRefusal::Exhausted {
                asked: 8,
                remaining: 4
            })
        );
    }

    #[test]
    fn turns_are_aggregate_across_children_not_per_child() {
        let mut active = ActiveEnvelope::new(SpawnEnvelope {
            max_provider_turns: 10,
            ..envelope()
        });
        active.draw(2, 8);
        assert_eq!(active.turns_remaining(), 2);
        assert_eq!(
            active.check_draw(1, 4, &[]),
            Err(EnvelopeRefusal::TurnsExhausted {
                asked: 4,
                remaining: 2
            })
        );
    }

    #[test]
    fn an_empty_route_list_covers_everything_and_a_named_one_does_not() {
        let open = ActiveEnvelope::new(envelope());
        assert!(open.check_draw(1, 1, &["smol".to_owned()]).is_ok());

        let narrow = ActiveEnvelope::new(SpawnEnvelope {
            routes: vec!["smol".to_owned()],
            ..envelope()
        });
        assert!(narrow.check_draw(1, 1, &["smol".to_owned()]).is_ok());
        assert_eq!(
            narrow.check_draw(1, 1, &["plan".to_owned()]),
            Err(EnvelopeRefusal::RouteNotCovered {
                route: "plan".to_owned()
            })
        );
    }

    #[test]
    fn an_envelope_lapses_on_the_turn_it_says_it_will() {
        let mut active = ActiveEnvelope::new(SpawnEnvelope {
            expires_after_turns: 3,
            ..envelope()
        });
        assert!(!active.tick());
        assert!(!active.tick());
        assert!(active.tick(), "the third turn is the last");
        assert!(active.expired());
    }
}
