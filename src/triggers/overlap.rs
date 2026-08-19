//! The overlap-policy decision, pure and unit-testable.
//!
//! Split out from the scheduler's async driver for the same reason
//! `runtime::subagent::clamp_policy` is a free function: the rule matters more
//! than the plumbing around it, and a pure function is where a test can pin it
//! down without standing up a runtime.

use crate::core::trigger::OverlapPolicy;

/// What the scheduler should do with an occurrence, given whether a firing is
/// already in flight and whether a queue slot is already held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapDecision {
    /// Nothing is in flight: start immediately.
    Start,
    /// Drop this occurrence; the in-flight firing keeps running.
    Skip,
    /// Hold this occurrence until the in-flight firing settles.
    Queue,
    /// Cancel the in-flight firing and start this occurrence.
    Replace,
}

/// Decide what to do with a new occurrence.
///
/// `already_queued` matters only for [`OverlapPolicy::Queue`]: at most one
/// occurrence is ever held ( forbids a workflow DSL, and an
/// unbounded queue is exactly the kind of scope that becomes one). A second
/// occurrence that arrives while one is already queued is skipped, not piled
/// up.
#[must_use]
pub const fn decide(
    policy: OverlapPolicy,
    in_flight: bool,
    already_queued: bool,
) -> OverlapDecision {
    if !in_flight {
        return OverlapDecision::Start;
    }
    match policy {
        OverlapPolicy::Skip => OverlapDecision::Skip,
        OverlapPolicy::Queue => {
            if already_queued {
                OverlapDecision::Skip
            } else {
                OverlapDecision::Queue
            }
        }
        OverlapPolicy::Replace => OverlapDecision::Replace,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_in_flight_always_starts() {
        for policy in [
            OverlapPolicy::Skip,
            OverlapPolicy::Queue,
            OverlapPolicy::Replace,
        ] {
            assert_eq!(decide(policy, false, false), OverlapDecision::Start);
            assert_eq!(decide(policy, false, true), OverlapDecision::Start);
        }
    }

    #[test]
    fn skip_drops_a_concurrent_occurrence() {
        assert_eq!(
            decide(OverlapPolicy::Skip, true, false),
            OverlapDecision::Skip
        );
    }

    #[test]
    fn queue_holds_exactly_one_occurrence_then_skips() {
        assert_eq!(
            decide(OverlapPolicy::Queue, true, false),
            OverlapDecision::Queue
        );
        assert_eq!(
            decide(OverlapPolicy::Queue, true, true),
            OverlapDecision::Skip,
            "a second occurrence while one is already queued must not pile up"
        );
    }

    #[test]
    fn replace_always_replaces_the_in_flight_firing() {
        assert_eq!(
            decide(OverlapPolicy::Replace, true, false),
            OverlapDecision::Replace
        );
        assert_eq!(
            decide(OverlapPolicy::Replace, true, true),
            OverlapDecision::Replace
        );
    }
}
