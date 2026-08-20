//! View state, reduced from runtime events.
//!
//! > "The TUI reduces `MjolnrEvent` values into view state and sends commands
//! > back. It cannot hold the authoritative session transcript."
//!
//! So this holds a *view*: finished messages come from the snapshot the runtime
//! publishes, and only the in-flight streaming text is assembled here — because
//! it does not exist anywhere else until the run finishes and the runtime
//! records the coalesced block.
//!
//! One reducer owns TUI state (AGENTS.md §2.3). Nothing else mutates it.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use crate::core::command::ApprovalDecision;
use crate::core::event::{FinishReason, MjolnrEvent};
#[cfg(test)]
use crate::core::message::ContentBlock;
use crate::core::model::QuotaSnapshot;
use crate::core::runtime::RuntimeSnapshot;
use crate::tui::keymap::KeymapState;

/// Keep paste/input growth bounded independently of provider limits. The
/// composer is view state and must not become an unbounded memory sink.
const MAX_COMPOSER_CHARS: usize = 64 * 1024;
const MAX_REASONING_CHARS: usize = 8 * 1024;

/// The first eight of a session id — enough to tell siblings apart on screen.
fn short_id(session: &crate::core::event::SessionId) -> String {
    let full = session.to_string();
    full.chars().take(8).collect()
}

/// View-side projection of the event vocabulary. It is never authoritative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    Waiting {
        on: &'static str,
    },
    Thinking,
    Responding,
    ToolAssembling {
        name: String,
    },
    ToolProposed {
        name: String,
        preview: String,
    },
    AwaitingApproval {
        name: String,
    },
    ToolRunning {
        name: String,
    },
    /// A child session's latest forwarded activity. The label
    /// is pre-formatted with the child's short id; only the newest is kept.
    Subagent {
        label: String,
    },
}

impl Activity {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Waiting { on } => format!("waiting on {on}"),
            Self::Thinking => "thinking".to_owned(),
            Self::Responding => "responding".to_owned(),
            Self::ToolAssembling { name } => format!("assembling {name}(…)"),
            Self::ToolProposed { name, .. } => format!("proposed {name}(…)"),
            Self::AwaitingApproval { name } => format!("waiting on approval for {name}(…)"),
            Self::ToolRunning { name } => format!("running {name}(…)"),
            Self::Subagent { label } => format!("subagent {label}"),
        }
    }
}

/// What the timeline shows for the current run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RunStatus {
    #[default]
    Idle,
    Streaming,
    Finished(FinishReasonView),
    Failed {
        code: String,
        detail: String,
    },
}

/// A display-safe finish reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReasonView {
    Stop,
    ToolCalls,
    Incomplete,
    Cancelled,
    Handoff,
    QuotaDrained,
}

/// One row in the fleet roster : a child agent — subagent or
/// council member — reduced from its forwarded `SubagentActivity`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetAgent {
    pub child: crate::core::event::SessionId,
    pub short: String,
    pub role: Option<String>,
    pub latest: String,
    /// The agent's own activity feed, oldest first — what Tab-into shows.
    pub feed: Vec<String>,
    pub done: bool,
    pub failed: bool,
    pub worktree_branch: Option<String>,
}

/// The one optional informational surface covering the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overlay {
    #[default]
    None,
    Help,
    Skills,
    Usage,
    Mcp,
    Triggers,
    Memory,
    Plugins,
    ExternalAgents,
    /// The model picker. Unlike the other overlays this one is interactive: it
    /// owns arrows and Enter while open (see `keymap::resolve_picker`).
    Models,
    /// Per-provider credential status.
    Auth,
    /// Tree explorer overlay
    Tree,
    /// Theme selector overlay
    Theme,
    /// Settings & Configuration surface overlay
    Config,
    /// The bounded result of an explicit repository discovery pass.
    Discovery,
}

/// A `/config` change staged for preview, not yet written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigStaged {
    /// Bind (or clear, with `None`) a route's persona — writes the route file.
    RoutePersona {
        route: String,
        persona: Option<String>,
    },
    /// Switch the active theme — writes the user theme config.
    Theme { theme: crate::tui::theme::ThemeId },
}

/// One editable row rendered by the `/config` surface: a label, the value in
/// effect now, and the staged value when this row is the one being changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigRow {
    pub label: String,
    pub current: String,
    pub staged: Option<String>,
    /// A one-line description of the file this row's write would touch, shown as
    /// the preview when a change is staged.
    pub writes: String,
}

impl ViewState {
    pub fn toggle_config(&mut self) {
        if self.overlay == Overlay::Config {
            self.overlay = Overlay::None;
            self.config_staged = None;
        } else {
            self.overlay = Overlay::Config;
            self.config_cursor = 0;
            self.config_staged = None;
        }
    }

    pub(crate) fn toggle_discovery(&mut self) {
        self.overlay = if self.overlay == Overlay::Discovery {
            Overlay::None
        } else {
            Overlay::Discovery
        };
    }

    /// The rows the `/config` surface shows: one per route (its persona
    /// binding) followed by the theme. Pure over the snapshot and theme state.
    pub(crate) fn config_rows(&self) -> Vec<ConfigRow> {
        let staged_route = match &self.config_staged {
            Some(ConfigStaged::RoutePersona { route, persona }) => {
                Some((route.clone(), persona.clone()))
            }
            _ => None,
        };
        let staged_theme = match &self.config_staged {
            Some(ConfigStaged::Theme { theme }) => Some(*theme),
            _ => None,
        };
        let mut rows: Vec<ConfigRow> = self
            .snapshot
            .routes
            .iter()
            .map(|route| {
                let staged = staged_route
                    .as_ref()
                    .filter(|(name, _)| name == &route.name)
                    .map(|(_, persona)| persona.clone().unwrap_or_else(|| "(none)".to_owned()));
                ConfigRow {
                    label: format!("route {} persona", route.name),
                    current: route.persona.clone().unwrap_or_else(|| "(none)".to_owned()),
                    staged,
                    writes: format!(".mjolnr/routes/{}.yaml", route.name),
                }
            })
            .collect();
        rows.push(ConfigRow {
            label: "theme".to_owned(),
            current: crate::tui::theme::active_theme_id().name().to_owned(),
            staged: staged_theme.map(|theme| theme.name().to_owned()),
            writes: "user theme config".to_owned(),
        });
        rows
    }

    pub fn move_config_cursor(&mut self, delta: isize) {
        // Moving off a row abandons a change staged on it: a preview belongs to
        // the row in focus, not to wherever the cursor wanders next.
        self.config_staged = None;
        let count = self.config_rows().len();
        if count == 0 {
            return;
        }
        let current = self.config_cursor.min(count - 1);
        self.config_cursor = if delta < 0 {
            current.saturating_sub(usize::try_from(-delta).unwrap_or(0))
        } else {
            current
                .saturating_add(usize::try_from(delta).unwrap_or(0))
                .min(count - 1)
        };
    }

    /// Cycle the focused row's value to the next candidate, staging it for
    /// preview. Cycling back to the value in effect clears the staging — there
    /// is nothing to write when nothing changed.
    pub fn cycle_config_value(&mut self) {
        // A route row cycles its persona; the row past the last route is theme.
        if let Some(route) = self.snapshot.routes.get(self.config_cursor) {
            let route_name = route.name.clone();
            let route_persona = route.persona.clone();
            // Candidate personas: "(none)" then every discovered persona.
            let mut candidates: Vec<Option<String>> = vec![None];
            candidates.extend(self.snapshot.personas.iter().map(|p| Some(p.name.clone())));
            // The value currently shown for this row is the staged one if set,
            // else the route's real binding.
            let shown = match &self.config_staged {
                Some(ConfigStaged::RoutePersona { route, persona }) if route == &route_name => {
                    persona.clone()
                }
                _ => route_persona.clone(),
            };
            let index = candidates.iter().position(|c| c == &shown).unwrap_or(0);
            let next = candidates
                .get((index + 1) % candidates.len())
                .cloned()
                .flatten();
            self.config_staged = (next != route_persona).then_some(ConfigStaged::RoutePersona {
                route: route_name,
                persona: next,
            });
            return;
        }
        let themes = crate::tui::theme::ThemeId::all();
        if themes.is_empty() {
            return;
        }
        let active = crate::tui::theme::active_theme_id();
        let shown = match &self.config_staged {
            Some(ConfigStaged::Theme { theme }) => *theme,
            _ => active,
        };
        let index = themes.iter().position(|t| *t == shown).unwrap_or(0);
        let Some(&next) = themes.get((index + 1) % themes.len()) else {
            return;
        };
        self.config_staged = (next != active).then_some(ConfigStaged::Theme { theme: next });
    }

    /// Take the staged change to apply it, clearing the staging. Returns `None`
    /// when nothing is staged — confirming an unchanged surface writes nothing.
    pub(crate) fn take_config_staged(&mut self) -> Option<ConfigStaged> {
        self.config_staged.take()
    }

    pub fn clear_config_staged(&mut self) {
        self.config_staged = None;
    }
}

impl From<FinishReason> for FinishReasonView {
    fn from(reason: FinishReason) -> Self {
        match reason {
            FinishReason::Stop => Self::Stop,
            FinishReason::ToolCalls => Self::ToolCalls,
            FinishReason::Incomplete => Self::Incomplete,
            FinishReason::Cancelled => Self::Cancelled,
            FinishReason::Handoff => Self::Handoff,
            FinishReason::QuotaDrained => Self::QuotaDrained,
        }
    }
}

impl FinishReasonView {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            // `Incomplete` is not success. Labelling it "done" would misreport
            // state (AGENTS.md §1.3): the model stopped early.
            Self::Stop => "done",
            Self::ToolCalls => "awaiting tools",
            Self::Incomplete => "incomplete — model stopped early",
            Self::Cancelled => "cancelled",
            Self::Handoff => "handoff checkpoint saved",
            Self::QuotaDrained => "quota reserve drained — handoff saved",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub number: usize,
    pub description: String,
    pub done: bool,
}

/// One user turn as `/tree` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// The durable event to rewind to, when there is one.
    ///
    /// `None` means the transcript carries this message but the record has no
    /// point to branch from — a checkpoint-seeded message. Such a row is shown,
    /// because it is part of the conversation, but it cannot be selected as a
    /// branch point, and the overlay says so rather than failing silently on
    /// [`Enter`](crate::tui::input::InputAction::PickerConfirm).
    pub sequence: Option<u64>,
    pub prompt: String,
    /// The reply this turn received, or `None` while it is still in flight.
    pub answer: Option<String>,
    /// Whether this turn is on the branch the session is following.
    ///
    /// Decides what [`Enter`](crate::tui::keymap::InputAction::PickerConfirm)
    /// means on the row: branching away from a turn you are on, or returning to
    /// a branch you left.
    pub on_active_branch: bool,
    /// How many turns deep this row sits, for indentation. Siblings share a
    /// depth; a branch point is where two rows share a parent.
    pub depth: usize,
}

/// Everything the view needs, and nothing it must not own.
#[allow(
    clippy::struct_excessive_bools,
    reason = "ViewState carries independent rendering options and active state flags rather than a single state machine"
)]
#[derive(Debug)]
pub struct ViewState {
    /// Authoritative history, borrowed from the runtime's snapshot. Never
    /// mutated here.
    pub snapshot: RuntimeSnapshot,
    /// Text of the run currently streaming. Lives only until the run ends, at
    /// which point the runtime's snapshot carries the finished message.
    pub streaming_text: String,
    pub status: RunStatus,
    pub composer: String,
    pub composer_cursor: usize,
    pub composer_scroll: usize,
    /// Informational overlay. Skills is a view over snapshot state, never a
    /// second skill registry.
    pub overlay: Overlay,
    /// Tool output remains visible by default, but can be collapsed when the
    /// transcript is noisy.
    pub show_tool_details: bool,
    /// Manual distance from the newest timeline content. Zero follows tail.
    pub timeline_scroll_from_bottom: u16,
    /// Set when the event feed dropped events. Surfaced rather than hidden: a
    /// silently incomplete view is a lie about state.
    pub lagged: bool,
    /// Most recent model transition or typed refusal. Provider-private state is
    /// explicitly called out rather than implied to migrate.
    pub model_notice: Option<String>,
    pub activity: Option<Activity>,
    pub run_started_at: Option<Instant>,
    pub phase_started_at: Option<Instant>,
    pub reasoning_text: String,
    pub reasoning_started_at: Option<Instant>,
    pub thought_for: Option<Duration>,
    pub quota: Option<QuotaSnapshot>,
    pub auto_allowed_side_effects: u64,
    pub last_intent: Option<String>,
    pub full_auto_armed: bool,
    /// An envelope typed but not yet confirmed. Held here
    /// rather than dispatched immediately so arming authority over
    /// not-yet-proposed spawns costs a deliberate keystroke.
    pub envelope_armed: Option<Box<crate::core::envelope::SpawnEnvelope>>,
    pub tick: u64,
    /// Highlighted row in the model picker, as an index into the *filtered*
    /// list. Clamped on every read rather than tracked across filter changes —
    /// a cursor that survives a filter it no longer matches selects the wrong
    /// model.
    pub model_cursor: usize,
    /// Stateful key semantics (currently the confirmed-exit window). This is
    /// view state so rendering and input resolution agree about what is armed.
    pub(crate) keymap: KeymapState,
    pub plan_steps: Vec<PlanStep>,
    pub composer_queue: Vec<String>,
    pub cancelling: bool,
    pub tree_cursor: usize,
    /// Highlighted row in the auth provider picker.
    pub auth_cursor: usize,
    /// Highlighted row in the theme picker.
    pub theme_cursor: usize,
    /// The live fleet roster , reduced from `SubagentActivity`
    /// — parent aside, one row per active child (subagent or council member).
    pub fleet: Vec<FleetAgent>,
    /// The fleet agent whose own activity feed the main pane is focused on
    /// (Tab-into). `None` shows the ordinary transcript.
    pub focused_agent: Option<usize>,
    /// Highlighted row in the `/config` settings surface.
    pub config_cursor: usize,
    /// A change staged in `/config` but not yet written. Its presence is the
    /// preview: the surface shows what file the write will touch, and nothing
    /// lands until it is confirmed. Discarding it leaves every file untouched.
    pub(crate) config_staged: Option<ConfigStaged>,
    /// Styled immutable transcript messages. Interior mutability is confined
    /// to the single-threaded render pass; runtime truth remains in snapshot.
    pub(crate) render_cache: RefCell<crate::tui::render_cache::RenderCache>,
    /// Encoded inline images for the current transcript width. Same interior
    /// mutability contract as `render_cache`: written only inside the
    /// single-threaded render pass, and empty until the terminal reports a
    /// graphics protocol — so a frame test never emits a graphics escape.
    pub(crate) images: RefCell<crate::tui::image::ImageStore>,
    /// Wrapped timeline height observed on the previous frame.
    pub(crate) last_timeline_height: std::cell::Cell<usize>,
    /// State for the universal Jump Palette (`Ctrl+P`).
    pub jump_state: crate::tui::jump_palette::JumpState,
    /// Currently focused workspace primary surface tab.
    pub active_surface: crate::tui::workspace_types::WorkspaceSurface,
    /// Viewport scroll intent engine state (`FollowOutput` vs `PinnedHistory`).
    pub viewport: crate::tui::viewport::ViewportState,
    /// Interactive operator attention queue state.
    pub attention_queue: crate::tui::workspace_types::AttentionQueue,
    /// State for the Structured Plan surface.
    pub plan_surface: crate::tui::plan_surface::PlanSurfaceState,
    /// State for the Changes / Unified Diff surface.
    pub changes_surface: crate::tui::changes_surface::ChangesSurfaceState,
    /// State for the Verification Evidence surface.
    pub verify_surface: crate::tui::verify_surface::VerifySurfaceState,
    /// State for the Quick Launcher dashboard.
    pub launcher: crate::tui::launcher::LauncherState,
    /// Whether the auxiliary inspector side panel (Alt+P) is visible.
    pub auxiliary_panel_visible: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            snapshot: RuntimeSnapshot::default(),
            streaming_text: String::new(),
            status: RunStatus::default(),
            composer: String::new(),
            composer_cursor: 0,
            composer_scroll: 0,
            overlay: Overlay::None,
            show_tool_details: true,
            timeline_scroll_from_bottom: 0,
            lagged: false,
            model_notice: None,
            activity: None,
            run_started_at: None,
            phase_started_at: None,
            reasoning_text: String::new(),
            reasoning_started_at: None,
            thought_for: None,
            quota: None,
            auto_allowed_side_effects: 0,
            last_intent: None,
            full_auto_armed: false,
            envelope_armed: None,
            tick: 0,
            model_cursor: 0,
            keymap: KeymapState::default(),
            plan_steps: Vec::new(),
            composer_queue: Vec::new(),
            cancelling: false,
            tree_cursor: 0,
            auth_cursor: 0,
            theme_cursor: 0,
            fleet: Vec::new(),
            focused_agent: None,
            config_cursor: 0,
            config_staged: None,
            render_cache: RefCell::new(crate::tui::render_cache::RenderCache::default()),
            images: RefCell::new(crate::tui::image::ImageStore::default()),
            last_timeline_height: std::cell::Cell::new(0),
            jump_state: crate::tui::jump_palette::JumpState::default(),
            active_surface: crate::tui::workspace_types::WorkspaceSurface::Work,
            viewport: crate::tui::viewport::ViewportState::default(),
            attention_queue: crate::tui::workspace_types::AttentionQueue::default(),
            plan_surface: crate::tui::plan_surface::PlanSurfaceState::default(),
            changes_surface: crate::tui::changes_surface::ChangesSurfaceState::default(),
            verify_surface: crate::tui::verify_surface::VerifySurfaceState::default(),
            launcher: crate::tui::launcher::LauncherState::default(),
            auxiliary_panel_visible: false,
        }
    }
}

impl ViewState {
    pub fn queue_composer(&mut self) {
        if !self.composer.is_empty() {
            self.composer_queue.push(self.composer.clone());
            self.composer.clear();
            self.composer_cursor = 0;
        }
    }

    pub fn recall_queued_composer(&mut self) {
        if let Some(queued) = self.composer_queue.pop() {
            self.composer = queued;
            self.composer_cursor = self.composer.chars().count();
        }
    }

    /// Return every queued follow-up to the composer, oldest first, ahead of
    /// whatever is already typed.
    ///
    /// Used on abort. Nothing is discarded: the queue empties into text the
    /// user can see, edit, and decide about, which is the difference between
    /// cancelling a turn and losing what you wrote for it.
    pub fn restore_queued_to_composer(&mut self) {
        if self.composer_queue.is_empty() {
            return;
        }
        let mut restored = std::mem::take(&mut self.composer_queue);
        if !self.composer.trim().is_empty() {
            restored.push(std::mem::take(&mut self.composer));
        }
        self.composer = restored.join("\n");
        self.composer_cursor = self.composer.chars().count();
    }

    /// Toggle visibility of the auxiliary inspector side panel.
    pub fn toggle_auxiliary_panel(&mut self) {
        self.auxiliary_panel_visible = !self.auxiliary_panel_visible;
    }

    /// Hide the auxiliary inspector side panel.
    pub fn hide_auxiliary_panel(&mut self) {
        self.auxiliary_panel_visible = false;
    }

    /// Fold one runtime event into the view.
    #[allow(
        clippy::too_many_lines,
        reason = "one flat event-to-view-state mapping;  added the routing/breaker arms alongside the existing ones"
    )]
    pub fn apply(&mut self, event: &MjolnrEvent) {
        match event {
            MjolnrEvent::RunStarted { .. } => {
                self.streaming_text.clear();
                self.status = RunStatus::Streaming;
                self.timeline_scroll_from_bottom = 0;
                self.keymap.disarm();
                let now = Instant::now();
                self.run_started_at = Some(now);
                self.set_activity_at(Activity::Waiting { on: "model" }, now);
                self.reasoning_text.clear();
                self.reasoning_started_at = None;
                self.thought_for = None;
                self.last_intent = None;
            }
            event @ (MjolnrEvent::TextDelta { .. }
            | MjolnrEvent::ReasoningDelta { .. }
            | MjolnrEvent::ToolAssembling { .. }) => self.apply_live_event(event),
            MjolnrEvent::RunFinished { reason, .. } => {
                // The finished message is in the snapshot now; drop the
                // in-flight copy rather than rendering it twice.
                self.streaming_text.clear();
                self.status = RunStatus::Finished((*reason).into());
                self.finish_activity();
            }
            MjolnrEvent::RunFailed { code, detail, .. } => {
                self.streaming_text.clear();
                self.status = RunStatus::Failed {
                    code: code.to_string(),
                    detail: format!("{detail} — {}", code.sentence()),
                };
                self.finish_activity();
            }
            // A resumed session's interrupted work arrives in the snapshot's
            // `recovery`, not here: a client that joined late or lagged would
            // miss the event, and a guard a subscriber can miss is not a guard.
            // Clearing the stream is still right — whatever was mid-render when
            // the process died is not this session's text.
            MjolnrEvent::RecoveryRequired { .. } => {
                self.streaming_text.clear();
                self.status = RunStatus::Idle;
            }
            MjolnrEvent::RecoveryResolved { .. } => self.status = RunStatus::Idle,
            MjolnrEvent::QuotaReported { snapshot, .. } => {
                self.quota = Some(snapshot.clone());
            }
            MjolnrEvent::QuotaBoundaryReached { reserve, .. } => {
                self.model_notice = Some(format!(
                    "QUOTA {:?} // basis {:?} // reset {:?}",
                    reserve.phase, reserve.basis, reserve.resets_at
                ));
                if reserve.phase == crate::core::continuation::QuotaReservePhase::Draining {
                    self.set_activity(Activity::Waiting {
                        on: "quota handoff",
                    });
                }
            }
            MjolnrEvent::HandoffCreated { handoff, .. } => {
                self.model_notice = Some(format!("HANDOFF SAVED — {}", handoff.id));
            }
            event @ (MjolnrEvent::ToolProposed { .. }
            | MjolnrEvent::ApprovalResolved { .. }
            | MjolnrEvent::ToolCompleted { .. }
            | MjolnrEvent::ToolFailed { .. }) => self.apply_tool_event(event),
            MjolnrEvent::BudgetExhausted { .. } => {
                self.model_notice = Some(
                    "BUDGET EXHAUSTED — no further provider or tool work was started".to_owned(),
                );
            }
            MjolnrEvent::ModelChanged {
                provider, model, ..
            } => {
                self.quota = None;
                self.model_notice = Some(format!(
                    "MODEL CHANGED — {provider}:{model} · provider-private reasoning and cache state were not migrated"
                ));
            }
            MjolnrEvent::ModelChangeRefused { code, detail, .. } => {
                self.model_notice = Some(format!("{code} — {detail}"));
            }
            // Shown, not swallowed. `PolicyChanged` is silent because the
            // human who caused it already knows; this one is the runtime
            // narrowing a policy *they* set, and the model's tier is the only
            // explanation for why the header no longer says what they chose.
            // It reuses the model notice deliberately: the cause is the model.
            MjolnrEvent::PolicyClamped {
                from, to, tier, ..
            } => {
                self.model_notice = Some(format!(
                    "policy {} → {} — this model is governed as {}",
                    from.label(),
                    to.label(),
                    tier.label()
                ));
            }
            MjolnrEvent::SessionCreated { .. } => self.quota = None,
            event @ (MjolnrEvent::SubagentSpawned { .. }
            | MjolnrEvent::SubagentResultLate { .. }
            | MjolnrEvent::ReadSetCollision { .. }
            | MjolnrEvent::SubagentActivity { .. }) => self.apply_subagent_event(event),
            MjolnrEvent::RouteSelected {
                child: None,
                route,
                position,
                provider,
                model,
                ..
            } => {
                self.model_notice = Some(format!(
                    "ROUTE SELECTED — {route}[{position}] · {provider}:{model}"
                ));
            }
            MjolnrEvent::RouteAdvanced {
                route,
                to_position,
                provider,
                model,
                condition,
                ..
            } => {
                self.model_notice = Some(format!(
                    "ROUTE ADVANCED — {route}[{to_position}] · {provider}:{model} · {}",
                    condition.label()
                ));
            }
            MjolnrEvent::RouteExhausted { route, condition, .. } => {
                self.model_notice = Some(format!(
                    "ROUTE EXHAUSTED — {route} · {}",
                    condition.label()
                ));
            }
            MjolnrEvent::BreakerStateChanged {
                provider, from, to, ..
            } => {
                self.model_notice = Some(format!(
                    "BREAKER {provider} — {} → {}",
                    from.label(),
                    to.label()
                ));
            }
            MjolnrEvent::MessageAppended { .. }
            | MjolnrEvent::UsageReported { .. }
            | MjolnrEvent::PolicyChanged { .. }
            | MjolnrEvent::FileSaved { .. }
            // A load is acknowledged through `snapshot.last_extension_load`, the
            // same way `/reload` reports through `snapshot.last_reload`, rather
            // than as a transcript entry.
            | MjolnrEvent::ExtensionLoaded { .. }
            | MjolnrEvent::SessionEnded { .. }
            // Trigger lifecycle events narrate a trigger's control session, a
            // different session from any this runtime instance's TUI has open.
            // The `/triggers` overlay reads `snapshot.triggers` instead, the
            // same pattern `/mcp` uses for `snapshot.mcp_servers`.
            | MjolnrEvent::TriggerFired { .. }
            | MjolnrEvent::TriggerSettled { .. }
            | MjolnrEvent::TriggerSkipped { .. }
            | MjolnrEvent::TriggerQueued { .. }
            | MjolnrEvent::TriggerReplaced { .. }
            | MjolnrEvent::TriggerDisabled { .. }
            | MjolnrEvent::TriggerRearmed { .. }
            // A child's route selection narrates the parent's transcript; the
            // child's own session shows its selection via its own event feed.
            | MjolnrEvent::RouteSelected { child: Some(_), .. }
            // The envelope's live state reaches the view on the snapshot, which
            // carries what remains rather than a tally the view would have to
            // rebuild. These are its durable record, read from the event log.
            | MjolnrEvent::SpawnEnvelopeArmed { .. }
            | MjolnrEvent::SpawnEnvelopeDrawn { .. }
            | MjolnrEvent::SpawnEnvelopeCleared { .. }
            | MjolnrEvent::PlanQuestionAsked { .. }
            | MjolnrEvent::PlanQuestionAnswered { .. }
            | MjolnrEvent::PlanProposed { .. }
            | MjolnrEvent::PlanReviewed { .. }
            | MjolnrEvent::PlanApproved { .. }
            | MjolnrEvent::PlanHandoffCreated { .. }
            | MjolnrEvent::CouncilReviewed { .. }
            | MjolnrEvent::PlanInterviewStarted { .. }
            | MjolnrEvent::PlanPrdProposed { .. }
            | MjolnrEvent::CouncilFindingDispositionRecorded { .. }
            // An amendment is a proposal a human reads and edits in the
            // desktop editor; the TUI has no editor to open it in.
            | MjolnrEvent::CouncilAmendmentProposed { .. }
            // Review threads are a desktop surface. The TUI
            // has no diff gutter to pin a note to, so it renders none rather
            // than inventing a second, weaker review vocabulary.
            | MjolnrEvent::ReviewNoteRecorded { .. }
            | MjolnrEvent::ReviewCommentAdded { .. }
            | MjolnrEvent::ReviewRequestSent { .. }
            | MjolnrEvent::ReviewRequestAnswered { .. }
            // Decision tickets and imported items are durable records the E5 board
            // will project (Tauri, design steps 3–4); there is no per-event
            // delta and no TUI board to render one into, so the view folds
            // nothing here.
            | MjolnrEvent::DecisionTicketOpened { .. }
            | MjolnrEvent::DecisionTicketResolved { .. }
            | MjolnrEvent::ImportedItemFetched { .. }
            | MjolnrEvent::ImportedItemRefreshed { .. }
            | MjolnrEvent::ImportedActRecorded { .. }
            | MjolnrEvent::ImportedCommentRecorded { .. } => {}
        }
        self.update_plan_steps();
    }

    fn apply_subagent_event(&mut self, event: &MjolnrEvent) {
        match event {
            MjolnrEvent::SubagentSpawned {
                child,
                policy,
                branch,
                ..
            } => {
                self.model_notice = Some(format!(
                    "SUBAGENT SPAWNED — {} on {branch} [{}]",
                    short_id(child),
                    policy.label()
                ));
            }
            MjolnrEvent::SubagentResultLate { child, .. } => {
                self.model_notice = Some(format!(
                    "LATE SUBAGENT RESULT — {} settled after its group",
                    short_id(child)
                ));
            }
            MjolnrEvent::ReadSetCollision {
                reader,
                writer,
                path,
                ..
            } => {
                self.model_notice = Some(format!(
                    "READ-SET COLLISION — {} read {path}; {} modified it",
                    short_id(reader),
                    short_id(writer)
                ));
            }
            MjolnrEvent::SubagentActivity { child, label, .. } => {
                self.set_activity(Activity::Subagent {
                    label: format!("{} {label}", short_id(child)),
                });
                self.apply_fleet_activity(*child, label);
            }
            _ => {}
        }
    }

    /// Fold one child's forwarded activity into the fleet roster. A new
    /// convocation (activity arriving when every known agent has finished)
    /// clears the previous roster first, so the rail reflects the run at hand.
    fn apply_fleet_activity(&mut self, child: crate::core::event::SessionId, label: &str) {
        let failed = label.starts_with("failed") || label.starts_with("error");
        let done = label == "finished" || failed;
        if !self.fleet.is_empty() && self.fleet.iter().all(|agent| agent.done) {
            self.fleet.clear();
            self.focused_agent = None;
        }
        if let Some(agent) = self.fleet.iter_mut().find(|agent| agent.child == child) {
            label.clone_into(&mut agent.latest);
            agent.feed.push(label.to_owned());
            agent.done = agent.done || done;
            agent.failed = agent.failed || failed;
        } else {
            self.fleet.push(FleetAgent {
                child,
                short: short_id(&child),
                role: None,
                latest: label.to_owned(),
                feed: vec![label.to_owned()],
                done,
                failed,
                worktree_branch: None,
            });
        }
    }

    /// Projects the live fleet roster as a core `FleetSummary`.
    #[must_use]
    pub fn fleet_summary(&self) -> crate::core::fleet::FleetSummary {
        let agents: Vec<crate::core::fleet::FleetAgentSummary> = self
            .fleet
            .iter()
            .map(|agent| {
                let status = if agent.failed {
                    crate::core::fleet::FleetAgentStatus::Failed {
                        reason: agent.latest.clone(),
                    }
                } else if agent.done {
                    crate::core::fleet::FleetAgentStatus::Completed
                } else {
                    crate::core::fleet::FleetAgentStatus::Running
                };
                crate::core::fleet::FleetAgentSummary {
                    child_session_id: agent.child,
                    short_name: agent.short.clone(),
                    role: agent.role.clone(),
                    status,
                    latest_activity: agent.latest.clone(),
                    feed: agent.feed.clone(),
                    worktree_branch: agent.worktree_branch.clone(),
                }
            })
            .collect();
        crate::core::fleet::FleetSummary::from_agents(agents)
    }

    /// Whether the fleet rail should show: a convocation of two or more agents
    /// with at least one still working. A solo session pays no chrome tax.
    #[must_use]
    pub fn fleet_visible(&self) -> bool {
        self.fleet.len() >= 2 && self.fleet.iter().any(|agent| !agent.done)
    }

    /// Cycle Tab-into focus: none → first agent → … → last → none. Only cycles
    /// while the rail is visible, so focus cannot strand on a hidden roster.
    pub fn cycle_fleet_focus(&mut self) {
        if !self.fleet_visible() {
            self.focused_agent = None;
            return;
        }
        self.focused_agent = match self.focused_agent {
            None if !self.fleet.is_empty() => Some(0),
            Some(index) if index + 1 < self.fleet.len() => Some(index + 1),
            _ => None,
        };
    }

    #[must_use]
    pub fn focused_fleet_agent(&self) -> Option<&FleetAgent> {
        self.focused_agent.and_then(|index| self.fleet.get(index))
    }

    fn apply_live_event(&mut self, event: &MjolnrEvent) {
        match event {
            MjolnrEvent::TextDelta { text, .. } => {
                self.collapse_reasoning();
                self.streaming_text.push_str(text);
                self.set_activity(Activity::Responding);
            }
            MjolnrEvent::ReasoningDelta { text, .. } => {
                if self.reasoning_started_at.is_none() {
                    self.reasoning_started_at = Some(Instant::now());
                }
                let remaining =
                    MAX_REASONING_CHARS.saturating_sub(self.reasoning_text.chars().count());
                self.reasoning_text.extend(text.chars().take(remaining));
                self.set_activity(Activity::Thinking);
            }
            MjolnrEvent::ToolAssembling { name, .. } => {
                self.collapse_reasoning();
                self.last_intent = Some(name.clone());
                self.set_activity(Activity::ToolAssembling { name: name.clone() });
            }
            _ => {}
        }
    }

    fn apply_tool_event(&mut self, event: &MjolnrEvent) {
        match event {
            MjolnrEvent::ToolProposed { approval, call, .. } => {
                self.collapse_reasoning();
                self.last_intent = Some(call.name.clone());
                let activity = if approval.is_some() {
                    Activity::AwaitingApproval {
                        name: call.name.clone(),
                    }
                } else {
                    Activity::ToolRunning {
                        name: call.name.clone(),
                    }
                };
                self.set_activity(activity);
            }
            MjolnrEvent::ApprovalResolved { decision, .. } => {
                if *decision == ApprovalDecision::Deny {
                    self.set_activity(Activity::Waiting { on: "model" });
                    return;
                }
                if *decision == ApprovalDecision::AutoByPolicy {
                    self.auto_allowed_side_effects =
                        self.auto_allowed_side_effects.saturating_add(1);
                }
                let name = self.last_intent.clone().unwrap_or_default();
                self.set_activity(Activity::ToolRunning { name });
            }
            MjolnrEvent::ToolCompleted { .. } | MjolnrEvent::ToolFailed { .. } => {
                self.set_activity(Activity::Waiting { on: "next step" });
            }
            _ => {}
        }
    }

    /// Replace the borrowed snapshot. Cheap: it shares the transcript by `Arc`.
    pub fn sync(&mut self, snapshot: RuntimeSnapshot) {
        self.snapshot = snapshot;
        if !self.snapshot.run_active {
            self.cancelling = false;
        }
        self.update_plan_steps();
    }

    pub fn update_plan_steps(&mut self) {
        self.plan_steps = self
            .snapshot
            .plan
            .as_ref()
            .and_then(|workflow| match &workflow.stage {
                crate::core::plan::PlanStage::Proposed { proposal }
                | crate::core::plan::PlanStage::Reviewed { proposal, .. }
                | crate::core::plan::PlanStage::Approved { proposal, .. }
                | crate::core::plan::PlanStage::IterateRequested { proposal, .. }
                | crate::core::plan::PlanStage::Rejected { proposal, .. }
                | crate::core::plan::PlanStage::Handoff { proposal, .. } => {
                    Some((workflow, proposal))
                }
                crate::core::plan::PlanStage::Idle
                | crate::core::plan::PlanStage::QuestionPending { .. } => None,
            })
            .map_or_else(Vec::new, |(_, proposal)| {
                proposal
                    .steps
                    .iter()
                    .map(|step| PlanStep {
                        number: step.index,
                        description: format!("{}: {}", step.title, step.description),
                        // Plan approval/handoff authorizes execution; neither
                        // proves the step's effect completed.
                        done: false,
                    })
                    .collect()
            });
    }

    /// Authoritative projection of all active and historic work items.
    #[must_use]
    pub fn project_work_items(&self) -> Vec<crate::tui::workspace_types::WorkItem> {
        let mut items = Vec::new();

        let session_str = self
            .snapshot
            .session
            .as_ref()
            .map_or_else(|| "none".to_string(), ToString::to_string);
        let provider_model = match (&self.snapshot.provider, &self.snapshot.model) {
            (Some(p), Some(m)) => format!("{p}/{m}"),
            (Some(p), None) => p.to_string(),
            _ => "none".to_string(),
        };
        let worktree_path = self
            .snapshot
            .workspace_root
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());

        let main_lifecycle =
            if self.snapshot.pending_approval.is_some() || self.snapshot.recovery.is_required() {
                crate::tui::workspace_types::WorkItemLifecycle::NeedsDecision
            } else if self.snapshot.run_active {
                crate::tui::workspace_types::WorkItemLifecycle::Active
            } else if !self.plan_steps.is_empty() {
                crate::tui::workspace_types::WorkItemLifecycle::Reviewing
            } else {
                crate::tui::workspace_types::WorkItemLifecycle::Active
            };

        items.push(crate::tui::workspace_types::WorkItem {
            id: session_str.clone(),
            title: format!("Session {}", &session_str[..8.min(session_str.len())]),
            kind: crate::tui::workspace_types::WorkItemKind::Session {
                session_id: session_str,
            },
            lifecycle: main_lifecycle,
            unread: false,
            created_at_ts: 0,
            updated_at_ts: 0,
            active_policy_mode: format!("{:?}", self.snapshot.policy),
            provider_model: provider_model.clone(),
            worktree_path,
        });

        for agent in &self.fleet {
            let child_lifecycle = if agent.done {
                crate::tui::workspace_types::WorkItemLifecycle::Verified
            } else {
                crate::tui::workspace_types::WorkItemLifecycle::Active
            };

            let child_id = agent.child.to_string();
            items.push(crate::tui::workspace_types::WorkItem {
                id: child_id.clone(),
                title: agent.short.clone(),
                kind: crate::tui::workspace_types::WorkItemKind::Subagent {
                    subagent_id: child_id,
                    parent_session_id: self
                        .snapshot
                        .session
                        .as_ref()
                        .map_or_else(|| "none".to_string(), ToString::to_string),
                },
                lifecycle: child_lifecycle,
                unread: agent.done,
                created_at_ts: 0,
                updated_at_ts: 0,
                active_policy_mode: format!("{:?}", self.snapshot.policy),
                provider_model: provider_model.clone(),
                worktree_path: None,
            });
        }

        items
    }

    /// Authoritative projection of items requiring operator decision.
    #[must_use]
    pub fn project_attention_items(&self) -> Vec<crate::tui::workspace_types::AttentionItem> {
        let mut items = Vec::new();

        let session_str = self
            .snapshot
            .session
            .as_ref()
            .map_or_else(|| "none".to_string(), ToString::to_string);

        if self.snapshot.recovery.is_required() {
            items.push(crate::tui::workspace_types::AttentionItem {
                id: format!("recovery-{session_str}"),
                work_item_id: session_str.clone(),
                priority: crate::tui::workspace_types::AttentionPriority::UncertainRecovery,
                title: "Interrupted Execution Recovery".to_string(),
                reason_code: "UNCERTAIN_SIDE_EFFECT".to_string(),
                exact_effect_summary: format!("{:?}", self.snapshot.recovery),
                timestamp: 0,
            });
        }

        if let Some(approval) = &self.snapshot.pending_approval {
            items.push(crate::tui::workspace_types::AttentionItem {
                id: format!("approval-{session_str}"),
                work_item_id: session_str,
                priority: crate::tui::workspace_types::AttentionPriority::ApprovalRequired,
                title: format!("Approval Required: {}", approval.tool_name),
                reason_code: "TOOL_APPROVAL_REQUIRED".to_string(),
                exact_effect_summary: approval.preview.clone(),
                timestamp: 0,
            });
        }

        items.sort_by_key(|item| item.priority);
        items
    }

    /// Record that the feed lagged and the view must resync.
    pub fn note_lagged(&mut self) {
        self.lagged = true;
    }

    /// Add typed or pasted text without allowing unbounded view-state growth.
    pub(crate) fn append_composer(&mut self, text: &str) {
        let used = self.composer.chars().count();
        let remaining = MAX_COMPOSER_CHARS.saturating_sub(used);
        let text_to_insert: String = text.chars().take(remaining).collect();
        let char_len = self.composer.chars().count();
        self.composer_cursor = self.composer_cursor.min(char_len);
        let mut new_composer = String::new();
        for (i, c) in self.composer.chars().enumerate() {
            if i == self.composer_cursor {
                new_composer.push_str(&text_to_insert);
            }
            new_composer.push(c);
        }
        if self.composer_cursor == char_len {
            new_composer.push_str(&text_to_insert);
        }
        self.composer = new_composer;
        self.composer_cursor += text_to_insert.chars().count();
    }

    pub(crate) fn delete_composer_character(&mut self) {
        if self.composer_cursor > 0 {
            let char_len = self.composer.chars().count();
            self.composer_cursor = self.composer_cursor.min(char_len);
            let mut new_composer = String::new();
            for (i, c) in self.composer.chars().enumerate() {
                if i != self.composer_cursor - 1 {
                    new_composer.push(c);
                }
            }
            self.composer = new_composer;
            self.composer_cursor -= 1;
        }
    }

    pub(crate) fn clear_composer(&mut self) {
        self.composer.clear();
        self.composer_cursor = 0;
        self.composer_scroll = 0;
    }

    /// Replace the composer wholesale, for command completion.
    pub(crate) fn set_composer(&mut self, text: &str) {
        self.composer.clear();
        self.composer_cursor = 0;
        self.composer_scroll = 0;
        self.append_composer(text);
    }

    pub(crate) fn move_cursor_left(&mut self) {
        if self.composer_cursor > 0 {
            self.composer_cursor -= 1;
        }
    }

    pub(crate) fn move_cursor_right(&mut self) {
        let char_len = self.composer.chars().count();
        if self.composer_cursor < char_len {
            self.composer_cursor += 1;
        }
    }

    pub(crate) fn move_cursor_word_left(&mut self) {
        let char_len = self.composer.chars().count();
        self.composer_cursor = self.composer_cursor.min(char_len);
        let chars: Vec<char> = self.composer.chars().collect();
        let mut idx = self.composer_cursor;
        while idx > 0 && !chars.get(idx - 1).is_some_and(|c| c.is_alphanumeric()) {
            idx -= 1;
        }
        while idx > 0 && chars.get(idx - 1).is_some_and(|c| c.is_alphanumeric()) {
            idx -= 1;
        }
        self.composer_cursor = idx;
    }

    pub(crate) fn move_cursor_word_right(&mut self) {
        let char_len = self.composer.chars().count();
        self.composer_cursor = self.composer_cursor.min(char_len);
        let chars: Vec<char> = self.composer.chars().collect();
        let mut idx = self.composer_cursor;
        while idx < char_len && chars.get(idx).is_some_and(|c| c.is_alphanumeric()) {
            idx += 1;
        }
        while idx < char_len && !chars.get(idx).is_some_and(|c| c.is_alphanumeric()) {
            idx += 1;
        }
        self.composer_cursor = idx;
    }

    pub(crate) fn move_to_line_start(&mut self) {
        let char_len = self.composer.chars().count();
        self.composer_cursor = self.composer_cursor.min(char_len);
        let chars: Vec<char> = self.composer.chars().collect();
        let mut idx = self.composer_cursor;
        while idx > 0 && chars.get(idx - 1).is_some_and(|c| *c != '\n') {
            idx -= 1;
        }
        self.composer_cursor = idx;
    }

    pub(crate) fn move_to_line_end(&mut self) {
        let char_len = self.composer.chars().count();
        self.composer_cursor = self.composer_cursor.min(char_len);
        let chars: Vec<char> = self.composer.chars().collect();
        let mut idx = self.composer_cursor;
        while idx < char_len && chars.get(idx).is_some_and(|c| *c != '\n') {
            idx += 1;
        }
        self.composer_cursor = idx;
    }

    pub(crate) fn delete_character_at_cursor(&mut self) {
        let char_len = self.composer.chars().count();
        self.composer_cursor = self.composer_cursor.min(char_len);
        if self.composer_cursor < char_len {
            let mut new_composer = String::new();
            for (i, c) in self.composer.chars().enumerate() {
                if i != self.composer_cursor {
                    new_composer.push(c);
                }
            }
            self.composer = new_composer;
        }
    }

    pub(crate) fn delete_word_backward(&mut self) {
        let char_len = self.composer.chars().count();
        self.composer_cursor = self.composer_cursor.min(char_len);
        let chars: Vec<char> = self.composer.chars().collect();
        let mut idx = self.composer_cursor;
        while idx > 0 && !chars.get(idx - 1).is_some_and(|c| c.is_alphanumeric()) {
            idx -= 1;
        }
        while idx > 0 && chars.get(idx - 1).is_some_and(|c| c.is_alphanumeric()) {
            idx -= 1;
        }
        let mut new_composer = String::new();
        for (i, c) in self.composer.chars().enumerate() {
            if i < idx || i >= self.composer_cursor {
                new_composer.push(c);
            }
        }
        self.composer = new_composer;
        self.composer_cursor = idx;
    }

    pub(crate) fn delete_word_forward(&mut self) {
        let char_len = self.composer.chars().count();
        self.composer_cursor = self.composer_cursor.min(char_len);
        let chars: Vec<char> = self.composer.chars().collect();
        let mut idx = self.composer_cursor;
        while idx < char_len && chars.get(idx).is_some_and(|c| c.is_alphanumeric()) {
            idx += 1;
        }
        while idx < char_len && !chars.get(idx).is_some_and(|c| c.is_alphanumeric()) {
            idx += 1;
        }
        let mut new_composer = String::new();
        for (i, c) in self.composer.chars().enumerate() {
            if i < self.composer_cursor || i >= idx {
                new_composer.push(c);
            }
        }
        self.composer = new_composer;
    }

    pub(crate) fn delete_to_line_start(&mut self) {
        let char_len = self.composer.chars().count();
        self.composer_cursor = self.composer_cursor.min(char_len);
        let chars: Vec<char> = self.composer.chars().collect();
        let mut idx = self.composer_cursor;
        while idx > 0 && chars.get(idx - 1).is_some_and(|c| *c != '\n') {
            idx -= 1;
        }
        let mut new_composer = String::new();
        for (i, c) in self.composer.chars().enumerate() {
            if i < idx || i >= self.composer_cursor {
                new_composer.push(c);
            }
        }
        self.composer = new_composer;
        self.composer_cursor = idx;
    }

    pub(crate) fn delete_to_line_end(&mut self) {
        let char_len = self.composer.chars().count();
        self.composer_cursor = self.composer_cursor.min(char_len);
        let chars: Vec<char> = self.composer.chars().collect();
        let mut idx = self.composer_cursor;
        while idx < char_len && chars.get(idx).is_some_and(|c| *c != '\n') {
            idx += 1;
        }
        let mut new_composer = String::new();
        for (i, c) in self.composer.chars().enumerate() {
            if i < self.composer_cursor || i >= idx {
                new_composer.push(c);
            }
        }
        self.composer = new_composer;
    }

    pub(crate) fn toggle_help(&mut self) {
        self.overlay = if self.overlay == Overlay::Help {
            Overlay::None
        } else {
            Overlay::Help
        };
    }

    /// Moves to the next primary surface.
    ///
    /// Which surface is showing is organisational state: reversible by the same
    /// keystroke that changed it, and never a decision the runtime acts on.
    pub(crate) fn next_surface(&mut self) {
        self.active_surface = self.active_surface.next();
    }

    /// Moves to the previous primary surface.
    pub(crate) fn previous_surface(&mut self) {
        self.active_surface = self.active_surface.previous();
    }

    /// Jumps straight to the operator attention queue.
    pub(crate) fn jump_to_attention(&mut self) {
        self.active_surface = crate::tui::workspace_types::WorkspaceSurface::Attention;
    }

    pub(crate) fn toggle_skills(&mut self) {
        self.overlay = if self.overlay == Overlay::Skills {
            Overlay::None
        } else {
            Overlay::Skills
        };
    }

    pub(crate) fn toggle_usage(&mut self) {
        self.overlay = if self.overlay == Overlay::Usage {
            Overlay::None
        } else {
            Overlay::Usage
        };
    }

    pub(crate) fn toggle_mcp(&mut self) {
        self.overlay = if self.overlay == Overlay::Mcp {
            Overlay::None
        } else {
            Overlay::Mcp
        };
    }

    pub fn toggle_memory(&mut self) {
        self.overlay = if self.overlay == Overlay::Memory {
            Overlay::None
        } else {
            Overlay::Memory
        };
    }

    pub fn toggle_plugins(&mut self) {
        self.overlay = if self.overlay == Overlay::Plugins {
            Overlay::None
        } else {
            Overlay::Plugins
        };
    }

    pub fn toggle_external_agents(&mut self) {
        self.overlay = if self.overlay == Overlay::ExternalAgents {
            Overlay::None
        } else {
            Overlay::ExternalAgents
        };
    }

    /// Open or close the model picker.
    ///
    /// Opening parks the cursor on the session's current model rather than the
    /// top of the list: the common act is switching *away from here*, and a
    /// cursor that starts elsewhere makes the current selection hard to find.
    pub(crate) fn toggle_models(&mut self) {
        if self.overlay == Overlay::Models {
            self.overlay = Overlay::None;
            return;
        }
        self.overlay = Overlay::Models;
        self.model_cursor = self
            .filtered_models()
            .iter()
            .position(|choice| {
                Some(&choice.descriptor.provider) == self.snapshot.provider.as_ref()
                    && Some(&choice.descriptor.id) == self.snapshot.model.as_ref()
            })
            .unwrap_or(0);
    }

    pub(crate) fn toggle_theme(&mut self) {
        if self.overlay == Overlay::Theme {
            self.overlay = Overlay::None;
            return;
        }
        self.overlay = Overlay::Theme;
        let active = crate::tui::theme::active_theme_id();
        self.theme_cursor = crate::tui::theme::ThemeId::all()
            .iter()
            .position(|id| *id == active)
            .unwrap_or(0);
    }

    pub(crate) fn move_theme_cursor(&mut self, delta: isize) {
        let count = crate::tui::theme::ThemeId::all().len();
        if count == 0 {
            return;
        }
        let current = self.theme_cursor;
        let next = if delta < 0 {
            current.saturating_sub(usize::try_from(-delta).unwrap_or(0))
        } else {
            current.saturating_add(usize::try_from(delta).unwrap_or(0))
        };
        self.theme_cursor = next.min(count.saturating_sub(1));
    }

    /// The rows `/tree` shows, one per user turn.
    ///
    /// One projection, read by the cursor, the renderer, and the confirm
    /// handler alike. They used to derive the list separately, which is how a
    /// cursor comes to index a row the renderer never drew — and once the
    /// cursor selects a rewind target, that disagreement stops being a cosmetic
    /// off-by-one and starts rewinding to the wrong message.
    ///
    /// Prefers the loaded session tree, which includes branches the session is
    /// no longer on. Until that arrives the transcript is the best available
    /// answer: it is the same rows minus the siblings, so the list does not
    /// change shape under the user when the tree lands — it only grows.
    #[must_use]
    pub(crate) fn tree_rows(&self) -> Vec<TreeRow> {
        if self.snapshot.tree.is_empty() {
            return self.tree_rows_from_transcript();
        }
        self.tree_rows_from_tree()
    }

    /// Depth-first over the loaded tree, so siblings sit under their shared
    /// parent and a branch point reads as one.
    fn tree_rows_from_tree(&self) -> Vec<TreeRow> {
        let nodes = &self.snapshot.tree;
        let mut children: std::collections::BTreeMap<Option<u64>, Vec<usize>> =
            std::collections::BTreeMap::new();
        for (index, node) in nodes.iter().enumerate() {
            children.entry(node.parent).or_default().push(index);
        }

        let mut rows = Vec::new();
        // Explicit stack rather than recursion: a long session is a deep tree,
        // and a stack overflow in the renderer would take the terminal with it.
        // Pushed in reverse so siblings come off the stack in sequence order.
        let mut stack: Vec<(usize, usize)> = children
            .get(&None)
            .into_iter()
            .flatten()
            .rev()
            .map(|index| (*index, 0))
            .collect();

        while let Some((index, depth)) = stack.pop() {
            let Some(node) = nodes.get(index) else {
                continue;
            };
            rows.push(TreeRow {
                sequence: Some(node.sequence),
                prompt: node.prompt.clone(),
                answer: node.answer.clone(),
                on_active_branch: node.on_active_branch,
                depth,
            });
            if let Some(kids) = children.get(&Some(node.sequence)) {
                stack.extend(kids.iter().rev().map(|child| (*child, depth + 1)));
            }
        }
        rows
    }

    /// The active branch alone, derived from the transcript already on the
    /// snapshot. Used before the tree loads.
    fn tree_rows_from_transcript(&self) -> Vec<TreeRow> {
        let messages = &self.snapshot.messages;
        let mut rows = Vec::new();
        for (index, entry) in messages.iter().enumerate() {
            if entry.role != crate::core::message::Role::User {
                continue;
            }
            // The reply to this turn, for context in the list. Stops at the next
            // user turn: a later answer belongs to a later question.
            let mut answer = None;
            for next in messages.iter().skip(index + 1) {
                match next.role {
                    crate::core::message::Role::Assistant => {
                        answer = Some(next.text());
                        break;
                    }
                    crate::core::message::Role::User => break,
                    _ => {}
                }
            }
            rows.push(TreeRow {
                sequence: entry.sequence,
                prompt: entry.text(),
                answer,
                // Everything in the transcript is, by definition, the branch
                // being followed.
                on_active_branch: true,
                depth: 0,
            });
        }
        rows
    }

    pub(crate) fn toggle_tree(&mut self) {
        if self.overlay == Overlay::Tree {
            self.overlay = Overlay::None;
            return;
        }
        self.overlay = Overlay::Tree;
        self.tree_cursor = self.tree_rows().len().saturating_sub(1);
    }

    /// The row the cursor is on, if there is one.
    #[must_use]
    pub(crate) fn selected_tree_row(&self) -> Option<TreeRow> {
        self.tree_rows().into_iter().nth(self.tree_cursor)
    }

    pub(crate) fn tree_cursor_up(&mut self) {
        if self.tree_cursor > 0 {
            self.tree_cursor -= 1;
        }
    }

    pub(crate) fn tree_cursor_down(&mut self) {
        if self.tree_cursor + 1 < self.tree_rows().len() {
            self.tree_cursor += 1;
        }
    }

    /// Choices matching the composer text, which doubles as the filter.
    ///
    /// Matching is a plain case-insensitive substring over `provider/model`, not
    /// a fuzzy score: a picker that reorders under you is worse than one that
    /// simply narrows.
    pub(crate) fn filtered_models(&self) -> Vec<&crate::core::runtime::ModelChoice> {
        let needle = self.composer.trim().to_lowercase();
        self.snapshot
            .models
            .iter()
            .filter(|choice| {
                if needle.is_empty() {
                    return true;
                }
                format!(
                    "{}/{}",
                    choice.descriptor.provider.as_str(),
                    choice.descriptor.id.as_str()
                )
                .to_lowercase()
                .contains(&needle)
            })
            .collect()
    }

    /// The highlighted choice, if the filtered list is non-empty.
    pub(crate) fn selected_model(&self) -> Option<&crate::core::runtime::ModelChoice> {
        let filtered = self.filtered_models();
        filtered
            .get(self.model_cursor.min(filtered.len().saturating_sub(1)))
            .copied()
    }

    pub(crate) fn move_model_cursor(&mut self, delta: isize) {
        let len = self.filtered_models().len();
        if len == 0 {
            self.model_cursor = 0;
            return;
        }
        // Saturating, not wrapping: arrowing past the end of a list should rest
        // at the end, not silently jump to the far side of it.
        let current = isize::try_from(self.model_cursor.min(len - 1)).unwrap_or(0);
        let next = (current + delta).clamp(0, isize::try_from(len - 1).unwrap_or(0));
        self.model_cursor = usize::try_from(next).unwrap_or(0);
    }

    pub(crate) fn toggle_auth(&mut self) {
        self.overlay = if self.overlay == Overlay::Auth {
            Overlay::None
        } else {
            self.auth_cursor = 0;
            Overlay::Auth
        };
    }

    /// Every registered provider, including disconnected and unavailable ones.
    pub(crate) fn auth_providers(&self) -> Vec<&crate::core::runtime::ProviderConnection> {
        self.snapshot.providers.iter().collect()
    }

    pub(crate) fn move_auth_cursor(&mut self, delta: isize) {
        let len = self.auth_providers().len();
        if len == 0 {
            self.auth_cursor = 0;
            return;
        }
        let current = isize::try_from(self.auth_cursor.min(len - 1)).unwrap_or(0);
        let next = (current + delta).clamp(0, isize::try_from(len - 1).unwrap_or(0));
        self.auth_cursor = usize::try_from(next).unwrap_or(0);
    }

    pub(crate) fn toggle_triggers(&mut self) {
        self.overlay = if self.overlay == Overlay::Triggers {
            Overlay::None
        } else {
            Overlay::Triggers
        };
    }

    pub(crate) fn animating(&self) -> bool {
        self.snapshot.run_active
            || self.snapshot.pending_approval.is_some()
            || self.full_auto_armed
            || self.envelope_armed.is_some()
    }

    pub(crate) fn advance_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    fn set_activity(&mut self, activity: Activity) {
        self.set_activity_at(activity, Instant::now());
    }

    fn set_activity_at(&mut self, activity: Activity, now: Instant) {
        if self.activity.as_ref() != Some(&activity) {
            self.phase_started_at = Some(now);
        }
        self.activity = Some(activity);
    }

    fn collapse_reasoning(&mut self) {
        if let Some(started) = self.reasoning_started_at.take() {
            self.thought_for = Some(started.elapsed());
        }
        self.reasoning_text.clear();
    }

    fn finish_activity(&mut self) {
        self.collapse_reasoning();
        self.activity = None;
        self.phase_started_at = None;
    }

    pub(crate) fn close_overlay(&mut self) {
        self.overlay = Overlay::None;
    }

    pub(crate) fn note_model_command_failure(&mut self, detail: &str) {
        self.model_notice = Some(detail.to_owned());
    }

    /// Report state without implying anything went wrong. Shares
    /// `model_notice`'s slot; the distinction is in the wording, not the
    /// channel.
    pub(crate) fn note_model_command_notice(&mut self, detail: &str) {
        self.model_notice = Some(detail.to_owned());
    }

    pub(crate) fn toggle_tool_details(&mut self) {
        self.show_tool_details = !self.show_tool_details;
    }

    pub(crate) fn scroll_up(&mut self) {
        self.timeline_scroll_from_bottom = self.timeline_scroll_from_bottom.saturating_add(5);
    }

    pub(crate) fn scroll_down(&mut self) {
        self.timeline_scroll_from_bottom = self.timeline_scroll_from_bottom.saturating_sub(5);
    }

    pub(crate) fn scroll_to_bottom(&mut self) {
        self.timeline_scroll_from_bottom = 0;
    }
}

#[cfg(test)]
#[allow(
    clippy::field_reassign_with_default,
    clippy::indexing_slicing,
    reason = "test setup and assertions over known-length fixtures"
)]
mod tests {
    use super::*;
    use crate::core::event::{RunId, SessionId};
    use crate::core::message::CanonicalMessage;

    fn delta(text: &str) -> MjolnrEvent {
        MjolnrEvent::TextDelta {
            session: SessionId::new(),
            run: RunId::new(),
            text: text.to_owned(),
        }
    }

    fn activity(child: SessionId, label: &str) -> MjolnrEvent {
        MjolnrEvent::SubagentActivity {
            session: SessionId::new(),
            run: RunId::new(),
            child,
            label: label.to_owned(),
        }
    }

    #[test]
    fn fleet_reduces_subagent_activity_and_tab_into_cycles_focus() {
        let mut view = ViewState::default();
        assert!(!view.fleet_visible(), "no agents, no rail");

        let a = SessionId::new();
        let b = SessionId::new();
        view.apply(&activity(a, "started"));
        assert!(
            !view.fleet_visible(),
            "one agent is still hidden (0–1 rule)"
        );
        view.apply(&activity(b, "started"));
        view.apply(&activity(a, "deliberating"));
        assert!(view.fleet_visible(), "two live agents show the rail");
        assert_eq!(view.fleet.len(), 2);
        assert_eq!(view.fleet[0].latest, "deliberating");
        assert_eq!(view.fleet[0].feed, vec!["started", "deliberating"]);

        // Tab-into cycles none → first → second → none.
        assert_eq!(view.focused_agent, None);
        view.cycle_fleet_focus();
        assert_eq!(view.focused_fleet_agent().map(|agent| agent.child), Some(a));
        view.cycle_fleet_focus();
        assert_eq!(view.focused_fleet_agent().map(|agent| agent.child), Some(b));
        view.cycle_fleet_focus();
        assert_eq!(view.focused_agent, None);

        // When every member finishes, the rail auto-hides.
        view.apply(&activity(a, "finished"));
        view.apply(&activity(b, "finished"));
        assert!(!view.fleet_visible(), "a settled council hides the rail");

        // A fresh convocation clears the settled roster.
        let c = SessionId::new();
        view.apply(&activity(c, "started"));
        assert_eq!(view.fleet.len(), 1, "the finished roster was cleared");
        assert_eq!(view.fleet[0].child, c);
    }

    fn config_view() -> ViewState {
        use crate::core::context::PersonaSummary;
        use crate::core::model::{ModelId, ProviderId};
        use crate::core::runtime::RouteChoice;
        let mut view = ViewState::default();
        view.snapshot = RuntimeSnapshot {
            routes: std::sync::Arc::new(vec![RouteChoice {
                name: "main".to_owned(),
                roles: vec!["default".to_owned()],
                provider: ProviderId::new("openai"),
                model: ModelId::new("gpt-5.4"),
                persona: None,
            }]),
            personas: std::sync::Arc::new(vec![
                PersonaSummary {
                    name: "mentor".to_owned(),
                    description: None,
                    scope: crate::core::context::SkillScope::Project,
                },
                PersonaSummary {
                    name: "critic".to_owned(),
                    description: None,
                    scope: crate::core::context::SkillScope::Project,
                },
            ]),
            ..RuntimeSnapshot::default()
        };
        view.overlay = Overlay::Config;
        view
    }

    #[test]
    fn config_cycle_stages_a_preview_without_writing_and_esc_discards_it() {
        let mut view = config_view();
        // The route row shows its real binding, nothing staged yet.
        assert_eq!(view.config_staged, None);
        assert_eq!(view.config_rows()[0].current, "(none)");
        assert!(view.config_rows()[0].staged.is_none());

        // Space cycles the persona to the first candidate: a staged preview,
        // not a write. Nothing on disk is touched by the reducer at all.
        view.cycle_config_value();
        assert_eq!(
            view.config_staged,
            Some(ConfigStaged::RoutePersona {
                route: "main".to_owned(),
                persona: Some("mentor".to_owned()),
            })
        );
        assert_eq!(view.config_rows()[0].staged.as_deref(), Some("mentor"));

        // Declining (Esc) discards the staged change; the row is unchanged.
        view.clear_config_staged();
        assert_eq!(view.config_staged, None);
        assert_eq!(view.config_rows()[0].current, "(none)");
    }

    #[test]
    fn config_cycles_through_every_persona_and_back_to_none() {
        let mut view = config_view();
        view.cycle_config_value();
        assert!(matches!(
            &view.config_staged,
            Some(ConfigStaged::RoutePersona { persona: Some(p), .. }) if p == "mentor"
        ));
        view.cycle_config_value();
        assert!(matches!(
            &view.config_staged,
            Some(ConfigStaged::RoutePersona { persona: Some(p), .. }) if p == "critic"
        ));
        // Past the last persona wraps to "(none)", which equals the real
        // binding, so the staging clears — nothing to write.
        view.cycle_config_value();
        assert_eq!(view.config_staged, None);
    }

    #[test]
    fn take_config_staged_returns_the_change_to_apply_and_clears_it() {
        let mut view = config_view();
        view.cycle_config_value();
        let taken = view.take_config_staged();
        assert!(taken.is_some());
        assert_eq!(view.config_staged, None, "applying consumes the staging");
    }

    #[test]
    fn streaming_text_accumulates_then_clears_when_the_run_ends() {
        let mut view = ViewState::default();
        let session = SessionId::new();
        let run = RunId::new();

        view.apply(&MjolnrEvent::RunStarted { session, run });
        view.apply(&delta("Hel"));
        view.apply(&delta("lo"));
        assert_eq!(view.streaming_text, "Hello");
        assert_eq!(view.status, RunStatus::Streaming);

        view.apply(&MjolnrEvent::RunFinished {
            session,
            run,
            reason: FinishReason::Stop,
        });
        // The runtime's snapshot now owns this text as a durable message.
        // Keeping it here too would render it twice.
        assert!(view.streaming_text.is_empty());
        assert_eq!(view.status, RunStatus::Finished(FinishReasonView::Stop));
    }

    #[test]
    fn incomplete_is_not_labelled_as_success() {
        assert_eq!(FinishReasonView::Stop.label(), "done");
        assert!(FinishReasonView::Incomplete.label().contains("incomplete"));
        assert_ne!(
            FinishReasonView::Incomplete.label(),
            FinishReasonView::Stop.label()
        );
    }

    #[test]
    fn a_failed_run_shows_its_stable_code() {
        let mut view = ViewState::default();
        view.apply(&MjolnrEvent::RunFailed {
            session: SessionId::new(),
            run: RunId::new(),
            code: crate::core::error::ReasonCode::ProviderAuth,
            detail: "bad key".to_owned(),
        });

        match &view.status {
            RunStatus::Failed { code, .. } => assert_eq!(code, "PROVIDER_AUTH"),
            other => panic!("expected a failed status, got {other:?}"),
        }
    }

    #[test]
    fn a_new_run_clears_the_previous_stream() {
        let mut view = ViewState::default();
        view.apply(&delta("stale"));
        view.apply(&MjolnrEvent::RunStarted {
            session: SessionId::new(),
            run: RunId::new(),
        });
        assert!(view.streaming_text.is_empty());
    }

    #[test]
    fn composer_input_is_bounded_and_unicode_safe() {
        let mut view = ViewState::default();
        view.append_composer(&"界".repeat(MAX_COMPOSER_CHARS + 1));

        assert_eq!(view.composer.chars().count(), MAX_COMPOSER_CHARS);
        view.delete_composer_character();
        assert_eq!(view.composer.chars().count(), MAX_COMPOSER_CHARS - 1);
    }

    fn choice(provider: &str, model: &str) -> crate::core::runtime::ModelChoice {
        crate::core::runtime::ModelChoice {
            descriptor: crate::core::model::ModelDescriptor {
                id: crate::core::model::ModelId::new(model),
                provider: crate::core::model::ProviderId::new(provider),
                display_name: model.to_owned(),
                capabilities: crate::core::model::ModelCapabilities::default(),
                context_tokens: None,
                max_output_tokens: None,
                tier: None,
            },
        }
    }

    fn view_with_models() -> ViewState {
        let mut view = ViewState::default();
        view.snapshot.models = std::sync::Arc::new(vec![
            choice("anthropic", "claude-opus-4-8"),
            choice("gemini", "gemini-3-pro"),
            choice("openai", "gpt-5.4"),
        ]);
        view
    }

    #[test]
    fn picker_opens_on_the_current_model() {
        let mut view = view_with_models();
        view.snapshot.provider = Some(crate::core::model::ProviderId::new("openai"));
        view.snapshot.model = Some(crate::core::model::ModelId::new("gpt-5.4"));

        view.toggle_models();

        assert_eq!(view.overlay, Overlay::Models);
        assert_eq!(view.model_cursor, 2);
    }

    #[test]
    fn composer_text_filters_the_picker() {
        let mut view = view_with_models();
        view.append_composer("GEM");

        let filtered = view.filtered_models();
        assert_eq!(filtered.len(), 1);
        assert_eq!(
            filtered
                .first()
                .expect("one match")
                .descriptor
                .provider
                .as_str(),
            "gemini"
        );
    }

    #[test]
    fn cursor_saturates_at_both_ends() {
        let mut view = view_with_models();
        view.toggle_models();

        view.move_model_cursor(-5);
        assert_eq!(view.model_cursor, 0);
        view.move_model_cursor(99);
        assert_eq!(view.model_cursor, 2);
    }

    #[test]
    fn selection_stays_in_range_when_a_filter_shrinks_the_list() {
        let mut view = view_with_models();
        view.toggle_models();
        view.move_model_cursor(2);
        // Cursor is at index 2, then the filter leaves a single row. Reading a
        // stale index here would select a model the user never highlighted.
        view.append_composer("gemini");

        let selected = view.selected_model().expect("a match exists");
        assert_eq!(selected.descriptor.provider.as_str(), "gemini");
    }

    #[test]
    fn no_plan_workflow_clears_stale_plan_steps() {
        let mut view = ViewState::default();
        view.plan_steps = vec![PlanStep {
            number: 1,
            description: "stale text-derived step".to_owned(),
            done: false,
        }];
        view.update_plan_steps();

        assert!(view.plan_steps.is_empty());
    }

    #[test]
    fn numbered_answer_without_a_plan_heading_is_not_an_execution_plan() {
        let mut view = ViewState::default();
        let text = "\
I am mjolnr, a local-first coding harness.

1. **Enforce strict standards** from `AGENTS.md`.
2. **Manage sessions and tools** across providers.
3. **Interact with the workspace** by reading and planning.
4. **Delegate bounded tasks** when asked.";
        let msg = CanonicalMessage {
            id: uuid::Uuid::now_v7(),
            role: crate::core::message::Role::Assistant,
            blocks: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            provider: None,
            model: None,
            created_at: time::OffsetDateTime::now_utc(),
        };
        view.snapshot = RuntimeSnapshot {
            messages: std::sync::Arc::new(vec![crate::core::message::TranscriptEntry::unanchored(
                msg,
            )]),
            ..RuntimeSnapshot::default()
        };

        view.update_plan_steps();

        assert!(
            view.plan_steps.is_empty(),
            "an ordinary numbered answer must not claim the execution-plan rail"
        );
    }

    #[test]
    fn snapshot_plan_drives_plan_steps_authoritatively() {
        let mut view = ViewState::default();
        let plan_id = crate::core::plan::PlanId::new();
        let mut workflow = crate::core::plan::PlanWorkflow::new(plan_id);

        let proposal = crate::core::plan::PlanProposal {
            plan_id,
            revision_id: crate::core::plan::RevisionId::new(1),
            title: "Authoritative Plan".to_string(),
            summary: "Plan summary".to_string(),
            steps: vec![
                crate::core::plan::PlanStep {
                    index: 1,
                    title: "Step 1".to_string(),
                    description: "First step".to_string(),
                },
                crate::core::plan::PlanStep {
                    index: 2,
                    title: "Step 2".to_string(),
                    description: "Second step".to_string(),
                },
            ],
            proposed_at: time::OffsetDateTime::now_utc(),
        };
        workflow.propose_plan(proposal).unwrap();

        view.snapshot.plan = Some(workflow);
        view.update_plan_steps();

        assert_eq!(view.plan_steps.len(), 2);
        assert_eq!(view.plan_steps[0].number, 1);
        assert_eq!(view.plan_steps[0].description, "Step 1: First step");
        assert!(!view.plan_steps[0].done);
    }

    #[test]
    fn a_new_revision_replaces_the_previous_projection() {
        let mut view = ViewState::default();
        let plan_id = crate::core::plan::PlanId::new();
        let mut workflow = crate::core::plan::PlanWorkflow::new(plan_id);
        let first = crate::core::plan::PlanProposal {
            plan_id,
            revision_id: crate::core::plan::RevisionId::new(1),
            title: "First".to_owned(),
            summary: String::new(),
            steps: vec![crate::core::plan::PlanStep {
                index: 1,
                title: "Old".to_owned(),
                description: "Superseded".to_owned(),
            }],
            proposed_at: time::OffsetDateTime::now_utc(),
        };
        workflow.propose_plan(first).unwrap();
        workflow
            .approve_plan(crate::core::plan::PlanApproval {
                plan_id,
                revision_id: crate::core::plan::RevisionId::new(1),
                approver: "Human".to_owned(),
                decision: crate::core::plan::ReviewVerdict::Iterate,
                note: Some("Revise".to_owned()),
                approved_at: time::OffsetDateTime::now_utc(),
            })
            .unwrap();
        workflow
            .propose_plan(crate::core::plan::PlanProposal {
                plan_id,
                revision_id: crate::core::plan::RevisionId::new(2),
                title: "Second".to_owned(),
                summary: String::new(),
                steps: vec![crate::core::plan::PlanStep {
                    index: 1,
                    title: "New".to_owned(),
                    description: "Current".to_owned(),
                }],
                proposed_at: time::OffsetDateTime::now_utc(),
            })
            .unwrap();

        view.snapshot.plan = Some(workflow);
        view.update_plan_steps();

        assert_eq!(view.plan_steps.len(), 1);
        assert_eq!(view.plan_steps[0].description, "New: Current");
        assert!(!view.plan_steps[0].done);
    }

    #[test]
    fn handed_off_plan_steps_do_not_claim_execution_completed() {
        let mut view = ViewState::default();
        let plan_id = crate::core::plan::PlanId::new();
        let revision_id = crate::core::plan::RevisionId::new(1);
        let mut workflow = crate::core::plan::PlanWorkflow::new(plan_id);
        workflow
            .propose_plan(crate::core::plan::PlanProposal {
                plan_id,
                revision_id,
                title: "Ready".to_owned(),
                summary: String::new(),
                steps: vec![crate::core::plan::PlanStep {
                    index: 1,
                    title: "Execute".to_owned(),
                    description: "Handed off".to_owned(),
                }],
                proposed_at: time::OffsetDateTime::now_utc(),
            })
            .unwrap();
        workflow
            .approve_plan(crate::core::plan::PlanApproval {
                plan_id,
                revision_id,
                approver: "Human".to_owned(),
                decision: crate::core::plan::ReviewVerdict::Approve,
                note: None,
                approved_at: time::OffsetDateTime::now_utc(),
            })
            .unwrap();
        workflow
            .handoff_plan(crate::core::plan::PlanHandoff {
                plan_id,
                revision_id,
                handoff_note: "Execute".to_owned(),
                created_at: time::OffsetDateTime::now_utc(),
            })
            .unwrap();

        view.snapshot.plan = Some(workflow);
        view.update_plan_steps();

        assert_eq!(view.plan_steps.len(), 1);
        assert!(!view.plan_steps[0].done);
    }

    /// A view over a fixed transcript, for the `/tree` projection tests.
    fn view_with(messages: Vec<crate::core::message::TranscriptEntry>) -> ViewState {
        ViewState {
            snapshot: RuntimeSnapshot {
                messages: std::sync::Arc::new(messages),
                ..RuntimeSnapshot::default()
            },
            ..ViewState::default()
        }
    }

    #[test]
    fn tree_rows_pair_each_user_turn_with_its_own_answer_and_anchor() {
        // What `/tree` selects from. The cursor indexes this list and the
        // confirm handler rewinds to the `sequence` on the row it lands on, so
        // a row paired with the wrong answer — or carrying the wrong anchor —
        // rewinds to the wrong message.
        use crate::core::message::{CanonicalMessage, TranscriptEntry};

        let view = view_with(vec![
            TranscriptEntry::anchored(1, CanonicalMessage::user("first")),
            TranscriptEntry::anchored(
                2,
                CanonicalMessage::assistant(
                    vec![ContentBlock::Text {
                        text: "answer one".to_owned(),
                    }],
                    crate::core::model::ProviderId::new("fake"),
                    crate::core::model::ModelId::new("fake-1"),
                ),
            ),
            // No answer yet: the next entry is another user turn.
            TranscriptEntry::anchored(7, CanonicalMessage::user("second")),
        ]);

        assert_eq!(
            view.tree_rows(),
            vec![
                TreeRow {
                    sequence: Some(1),
                    prompt: "first".to_owned(),
                    answer: Some("answer one".to_owned()),
                    on_active_branch: true,
                    depth: 0,
                },
                TreeRow {
                    sequence: Some(7),
                    prompt: "second".to_owned(),
                    // An unanswered turn must not borrow a later turn's answer.
                    answer: None,
                    on_active_branch: true,
                    depth: 0,
                },
            ]
        );
    }

    #[test]
    fn the_cursor_selects_the_row_the_renderer_drew() {
        use crate::core::message::{CanonicalMessage, TranscriptEntry};

        let mut view = view_with(vec![
            TranscriptEntry::anchored(1, CanonicalMessage::user("first")),
            TranscriptEntry::anchored(4, CanonicalMessage::user("second")),
        ]);

        // Opening parks the cursor on the newest turn.
        view.toggle_tree();
        assert_eq!(
            view.selected_tree_row().map(|row| row.sequence),
            Some(Some(4))
        );

        view.tree_cursor_up();
        assert_eq!(
            view.selected_tree_row().map(|row| row.sequence),
            Some(Some(1))
        );

        // And the cursor cannot walk off either end into a row that is not there.
        view.tree_cursor_up();
        assert_eq!(view.tree_cursor, 0);
        view.tree_cursor_down();
        view.tree_cursor_down();
        assert_eq!(view.tree_cursor, 1);
    }

    #[test]
    fn a_checkpoint_seeded_turn_is_listed_but_offers_no_branch_point() {
        // It is part of the conversation, so it is shown; the record has no
        // event to branch from, so `sequence` is absent and the overlay marks
        // it rather than letting Enter fail silently.
        use crate::core::message::{CanonicalMessage, TranscriptEntry};

        let view = view_with(vec![TranscriptEntry::unanchored(CanonicalMessage::user(
            "restored",
        ))]);

        assert_eq!(
            view.tree_rows(),
            vec![TreeRow {
                sequence: None,
                prompt: "restored".to_owned(),
                answer: None,
                on_active_branch: true,
                depth: 0,
            }]
        );
    }

    fn node(
        sequence: u64,
        parent: Option<u64>,
        prompt: &str,
        on_active_branch: bool,
    ) -> crate::core::store::SessionTreeNode {
        crate::core::store::SessionTreeNode {
            sequence,
            parent,
            prompt: prompt.to_owned(),
            answer: None,
            on_active_branch,
        }
    }

    fn view_with_tree(tree: Vec<crate::core::store::SessionTreeNode>) -> ViewState {
        ViewState {
            snapshot: RuntimeSnapshot {
                tree: std::sync::Arc::new(tree),
                ..RuntimeSnapshot::default()
            },
            ..ViewState::default()
        }
    }

    #[test]
    fn siblings_render_under_their_shared_parent_at_the_same_depth() {
        // The shape that makes `/tree` a tree rather than a list: turn 1 was
        // answered twice, once on the branch that was abandoned and once on the
        // one being followed. Both must be visible, indented together, so the
        // branch point reads as a branch point.
        let view = view_with_tree(vec![
            node(1, None, "start", true),
            node(3, Some(1), "abandoned", false),
            node(5, Some(1), "current", true),
            node(7, Some(5), "continued", true),
        ]);

        let rows = view.tree_rows();
        let shape: Vec<(u64, usize, bool)> = rows
            .iter()
            .filter_map(|row| Some((row.sequence?, row.depth, row.on_active_branch)))
            .collect();

        assert_eq!(
            shape,
            vec![(1, 0, true), (3, 1, false), (5, 1, true), (7, 2, true)],
            "siblings share a depth; a turn that follows one sits deeper"
        );
    }

    #[test]
    fn an_empty_tree_falls_back_to_the_transcript_rather_than_showing_nothing() {
        // Empty means "not loaded yet", not "no history". Showing nothing would
        // make `/tree` look broken for the moment between opening and the read
        // landing — and the transcript is the same rows minus the siblings, so
        // the list grows rather than changing shape.
        use crate::core::message::{CanonicalMessage, TranscriptEntry};

        let view = view_with(vec![TranscriptEntry::anchored(
            2,
            CanonicalMessage::user("said"),
        )]);

        assert!(view.snapshot.tree.is_empty());
        let rows = view.tree_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows.first().map(|row| row.sequence), Some(Some(2)));
        assert_eq!(
            rows.first().map(|row| row.on_active_branch),
            Some(true),
            "the transcript is the active branch by definition"
        );
    }
}
