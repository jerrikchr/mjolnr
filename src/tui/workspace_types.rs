//! Domain projections for the governed agent workspace design system.
//!
//! These types provide read-only views over runtime state and events to drive
//! the spatial navigation shell, work lifecycle tracking, attention queue,
//! explicit plan surfaces, and viewport scroll intent.

/// Canonical lifecycle state of a work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkItemLifecycle {
    Draft,
    Active,
    NeedsDecision,
    Reviewing,
    Verified,
    Failed,
    Archived,
}

impl WorkItemLifecycle {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Draft => "Draft",
            Self::Active => "Active",
            Self::NeedsDecision => "Needs Decision",
            Self::Reviewing => "Reviewing",
            Self::Verified => "Verified",
            Self::Failed => "Failed",
            Self::Archived => "Archived",
        }
    }

    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Draft => "[ ⚪ Draft ]",
            Self::Active => "[ 🟢 Active ]",
            Self::NeedsDecision => "[ 🟡 Needs Decision ]",
            Self::Reviewing => "[ 🔵 Reviewing ]",
            Self::Verified => "[ 🟢 Verified ]",
            Self::Failed => "[ 🔴 Failed ]",
            Self::Archived => "[ 📁 Archived ]",
        }
    }
}

/// Type of work item represented in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkItemKind {
    Session {
        session_id: String,
    },
    Subagent {
        subagent_id: String,
        parent_session_id: String,
    },
    CouncilMember {
        member_id: String,
        task_id: String,
    },
    BranchFork {
        branch_name: String,
        base_session_id: String,
    },
}

/// Authoritative projection of a work item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItem {
    pub id: String,
    pub title: String,
    pub kind: WorkItemKind,
    pub lifecycle: WorkItemLifecycle,
    pub unread: bool,
    pub created_at_ts: u64,
    pub updated_at_ts: u64,
    pub active_policy_mode: String,
    pub provider_model: String,
    pub worktree_path: Option<String>,
}

/// Strict priority ordering for items requiring operator decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AttentionPriority {
    DurabilityLost = 1,
    UncertainRecovery = 2,
    ApprovalRequired = 3,
    VerificationFailed = 4,
    QuotaBudgetStop = 5,
    CompletedUnread = 6,
    InformationalNotice = 7,
}

impl AttentionPriority {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::DurabilityLost => "Durability Lost",
            Self::UncertainRecovery => "Uncertain Recovery",
            Self::ApprovalRequired => "Approval Required",
            Self::VerificationFailed => "Verification Failed",
            Self::QuotaBudgetStop => "Quota Stop",
            Self::CompletedUnread => "Completed Unread",
            Self::InformationalNotice => "Notice",
        }
    }
}

/// Item waiting in the operator's attention queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionItem {
    pub id: String,
    pub work_item_id: String,
    pub priority: AttentionPriority,
    pub title: String,
    pub reason_code: String,
    pub exact_effect_summary: String,
    pub timestamp: u64,
}

/// Viewport scroll intent engine state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ViewportIntent {
    #[default]
    FollowOutput,
    PinnedHistory {
        line_offset: usize,
    },
}

/// Primary task-oriented workspace surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WorkspaceSurface {
    Work,
    #[default]
    Conversation,
    Plan,
    Changes,
    Verify,
    Attention,
}

impl WorkspaceSurface {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Work => "Work",
            Self::Conversation => "Conversation",
            Self::Plan => "Plan",
            Self::Changes => "Changes",
            Self::Verify => "Verify",
            Self::Attention => "Attention",
        }
    }

    /// Parses a surface back from its own [`label`](Self::label).
    ///
    /// The jump palette carries its target as text because a jump item may
    /// point at a surface, a file, or a command. This is the one place that
    /// text becomes a surface again, so an unknown label resolves to `None`
    /// rather than to a default that would navigate somewhere unasked.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "Work" => Some(Self::Work),
            "Conversation" => Some(Self::Conversation),
            "Plan" => Some(Self::Plan),
            "Changes" => Some(Self::Changes),
            "Verify" => Some(Self::Verify),
            "Attention" => Some(Self::Attention),
            _ => None,
        }
    }

    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Work => Self::Conversation,
            Self::Conversation => Self::Plan,
            Self::Plan => Self::Changes,
            Self::Changes => Self::Verify,
            Self::Verify => Self::Attention,
            Self::Attention => Self::Work,
        }
    }

    #[must_use]
    pub fn previous(self) -> Self {
        match self {
            Self::Work => Self::Attention,
            Self::Conversation => Self::Work,
            Self::Plan => Self::Conversation,
            Self::Changes => Self::Plan,
            Self::Verify => Self::Changes,
            Self::Attention => Self::Verify,
        }
    }
}

/// Interactive state container for the Operator Attention Queue.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttentionQueue {
    pub selected_index: usize,
    pub active: bool,
}

impl AttentionQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn move_cursor_up(&mut self, total: usize) {
        if total == 0 {
            self.selected_index = 0;
            return;
        }
        if self.selected_index == 0 {
            self.selected_index = total.saturating_sub(1);
        } else {
            self.selected_index = self.selected_index.saturating_sub(1);
        }
    }

    pub fn move_cursor_down(&mut self, total: usize) {
        if total == 0 {
            self.selected_index = 0;
            return;
        }
        if self.selected_index + 1 >= total {
            self.selected_index = 0;
        } else {
            self.selected_index = self.selected_index.saturating_add(1);
        }
    }
}

/// Status of an individual step in a structured plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlanStepState {
    #[default]
    Pending,
    InProgress,
    Completed,
    Failed,
    Refused,
}

/// Individual step within a structured plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredPlanStep {
    pub number: usize,
    pub description: String,
    pub state: PlanStepState,
}

/// Explicit, structured plan emitted by runtime events or formal proposals.
///
/// Displaces transcript prose regex matching.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructuredPlan {
    pub id: String,
    pub title: String,
    pub approved: bool,
    pub steps: Vec<StructuredPlanStep>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_SURFACES: [WorkspaceSurface; 6] = [
        WorkspaceSurface::Work,
        WorkspaceSurface::Conversation,
        WorkspaceSurface::Plan,
        WorkspaceSurface::Changes,
        WorkspaceSurface::Verify,
        WorkspaceSurface::Attention,
    ];

    #[test]
    fn every_surface_label_parses_back_to_its_surface() {
        for surface in ALL_SURFACES {
            assert_eq!(WorkspaceSurface::from_label(surface.label()), Some(surface));
        }
    }

    #[test]
    fn an_unknown_label_navigates_nowhere() {
        assert_eq!(WorkspaceSurface::from_label("Conversations"), None);
        assert_eq!(WorkspaceSurface::from_label(""), None);
        assert_eq!(WorkspaceSurface::from_label("work"), None);
    }

    #[test]
    fn next_and_previous_are_inverses_across_the_whole_cycle() {
        for surface in ALL_SURFACES {
            assert_eq!(surface.next().previous(), surface);
            assert_eq!(surface.previous().next(), surface);
        }
    }

    #[test]
    fn cycling_forward_visits_every_surface_before_repeating() {
        let mut seen = vec![WorkspaceSurface::Work];
        let mut current = WorkspaceSurface::Work;
        for _ in 0..5 {
            current = current.next();
            assert!(!seen.contains(&current), "{current:?} repeated early");
            seen.push(current);
        }
        assert_eq!(current.next(), WorkspaceSurface::Work);
    }
}
