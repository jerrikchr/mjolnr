//! TUI frame tests using Ratatui's `TestBackend` .
//!
//! These assert **stable semantic content**, not every colour cell. A test that
//! pins exact styling breaks on every cosmetic change and gets deleted; a test
//! that pins meaning survives and keeps paying.
//!
//! No terminal, no runtime, no provider: `layout::render` is pure, which is what
//! makes this possible at all.

#![allow(clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use smed::core::command::ApprovalId;
use smed::core::context::{ContextDiagnostic, SkillScope, SkillSummary};
use smed::core::error::ReasonCode;
use smed::core::event::{FinishReason, RunId, SessionId, SmedEvent};
use smed::core::message::{CanonicalMessage, ContentBlock, ToolCall, ToolEffect, ToolResult};
use smed::core::model::{
    ModelCapabilities, ModelDescriptor, ModelId, ProviderId, QuotaSnapshot, QuotaWindow,
};
use smed::core::policy::PendingApproval;
use smed::core::runtime::RuntimeSnapshot;
use smed::core::tool::ToolTier;
use smed::tui::layout;
use smed::tui::reducer::{Overlay, ViewState};

/// Render at a given size and return the frame as text.
fn render_at(width: u16, height: u16, view: &ViewState) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal
        .draw(|frame| layout::render(frame, view))
        .expect("draw");

    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

/// Build a view whose transcript is anchored the way a real one is: one
/// durable event per message, so `/tree` sees selectable branch points.
fn view_with_messages(messages: Vec<CanonicalMessage>) -> ViewState {
    let messages: Vec<smed::core::message::TranscriptEntry> = messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            smed::core::message::TranscriptEntry::anchored(index as u64, message)
        })
        .collect();
    let mut view = ViewState::default();
    view.sync(RuntimeSnapshot {
        envelope: None,
        envelope_refusal: None,
        session: Some(SessionId::new()),
        provider: Some(ProviderId::new("fake")),
        model: Some(ModelId::new("fake-1")),
        messages: Arc::new(messages),
        tree: Arc::new(Vec::new()),
        left_branch: None,
        run_active: false,
        usage: smed::core::model::Usage {
            input_tokens: 12,
            output_tokens: 34,
        },
        workspace_root: Some(std::path::PathBuf::from("/tmp/demo-project")),
        policy: smed::core::policy::PolicyMode::Ask,
        pending_approval: None,
        budget: smed::core::runtime::BudgetStatus::default(),
        recovery: smed::core::recovery::RecoveryState::Clean,
        store_failure: None,
        skills: Arc::new(Vec::new()),
        prompts: Arc::new(Vec::new()),
        extensions: Arc::new(Vec::new()),
        last_reload: None,
        last_extension_load: None,
        last_discovery: None,
        last_council: None,
        last_council_amendment: None,
        activated_skills: Arc::new(Vec::new()),
        context_diagnostics: Arc::new(Vec::new()),
        workspace_trusted: false,
        handoff: None,
        quota_reserve: smed::core::continuation::QuotaReserveStatus::default(),
        quota: None,
        resume_advice: None,
        mcp_servers: Arc::new(Vec::new()),
        triggers: Arc::new(Vec::new()),
        route: None,
        breakers: Arc::new(Vec::new()),
        providers: Arc::new(Vec::new()),
        models: Arc::new(Vec::new()),
        routes: Arc::new(Vec::new()),
        personas: Arc::new(Vec::new()),
        active_persona: None,
        souls: Arc::new(Vec::new()),
        sessions: Arc::new(Vec::new()),
        plan: None,
        repository: smed::core::repository::RepositoryView::NoProject,
        changes: smed::core::change_capture::ChangeView::NoProject,
        read_evidence: Arc::new(Vec::new()),
        review_threads: Arc::new(Vec::new()),
        memory: Arc::default(),
        plugins: Arc::new(Vec::new()),
        fleet: Arc::default(),
        preview: Arc::default(),
        external_agents: Vec::new(),
        external_agent_capability: smed::core::client::external_agent::ExternalAgentCapability {
            available: false,
            reason: None,
        },
    });
    view
}

#[test]
fn header_shows_project_provider_model_and_usage() {
    let view = view_with_messages(vec![]);
    let frame = render_at(80, 20, &view);

    assert!(
        frame.contains("demo-project"),
        "project name missing:\n{frame}"
    );
    assert!(frame.contains("fake-1"), "model missing:\n{frame}");
    assert!(frame.contains("12 in"), "input usage missing:\n{frame}");
    assert!(frame.contains("34 out"), "output usage missing:\n{frame}");
}

#[test]
fn empty_timeline_shows_only_the_mjolnr_identity() {
    let view = view_with_messages(vec![]);
    let frame = render_at(80, 20, &view);

    assert!(frame.contains("mjolnr"), "frame:\n{frame}");
    assert!(
        !frame.contains("MODEL PROPOSES // CODE DISPOSES")
            && !frame.contains("LOCAL-FIRST CODING AGENT")
            && !frame.contains("WELCOME TO SMED")
            && !frame.contains("START WITH A DIRECTIVE"),
        "the identity should stand alone; controls already live beside the composer: {frame}"
    );
}

/// Bottom chrome costs the rows it uses and no more.
///
/// The shell reserved four rows for a status band that draws two, directly
/// above a composer band fixed at four rows that drew one line of an empty
/// draft. Eight rows of a 30-row terminal went to chrome, half of it blank.
#[test]
fn an_idle_screen_spends_no_rows_on_blank_chrome() {
    let view = view_with_messages(vec![]);
    let frame = render_at(100, 30, &view);
    let rows: Vec<&str> = frame.lines().collect();

    let telemetry = rows
        .iter()
        .position(|row| row.contains("PROJECT"))
        .expect("telemetry row present");

    assert!(
        rows.len() - telemetry <= 4,
        "chrome grew back past the rows it draws:\n{frame}"
    );
    for row in rows.iter().skip(telemetry) {
        assert!(
            !row.trim().is_empty(),
            "blank row inside the chrome band:\n{frame}"
        );
    }
}

/// The wordmark appears once.
///
/// It was printed by the shell's navigation bar and again by the telemetry row
/// fourteen rows below it, which reads as two applications sharing a screen.
#[test]
fn the_telemetry_row_does_not_reprint_the_wordmark() {
    let mut view = view_with_messages(vec![]);
    view.snapshot.messages = Arc::new(vec![smed::core::message::TranscriptEntry::anchored(
        0,
        CanonicalMessage::user("hello"),
    )]);

    let frame = render_at(100, 30, &view);
    let telemetry = frame
        .lines()
        .find(|row| row.contains("PROJECT"))
        .expect("telemetry row present");

    assert!(
        !telemetry.contains("mjolnr"),
        "wordmark reprinted on the telemetry row: {telemetry}"
    );
}

#[test]
fn a_tall_empty_workspace_uses_the_quick_launcher() {
    let view = view_with_messages(vec![]);
    let frame = render_at(100, 30, &view);

    assert!(
        frame.contains("mjolnr") || frame.contains("MJOLNR"),
        "quick launcher missing:\n{frame}"
    );
    for preset in smed::tui::launcher::PRESETS {
        assert!(
            frame.contains(preset.name),
            "preset {} missing:\n{frame}",
            preset.name
        );
    }
}

#[test]
fn a_wide_empty_workspace_shows_launcher_configuration_and_controls() {
    let view = view_with_messages(vec![]);
    let frame = render_at(160, 40, &view);

    assert!(
        frame.contains("Session"),
        "launcher configuration missing:\n{frame}"
    );
    assert!(
        frame.contains("apply policy"),
        "launcher controls missing:\n{frame}"
    );
}

/// The launcher reports the session it is in, not an example of one.
///
/// The Phase UX 4 launcher hardcoded `claude-3-5-sonnet` and `gpt-4o` as preset
/// strings and displayed them as `Target Model` while the status bar one row
/// below correctly read `no-model`. Reported state is derived from the runtime
/// snapshot or it is not reported (`AGENTS.md` §1.3).
#[test]
fn the_launcher_never_names_a_model_the_session_is_not_configured_for() {
    let mut view = view_with_messages(vec![]);
    view.snapshot.provider = None;
    view.snapshot.model = None;
    view.snapshot.workspace_root = None;

    let frame = render_at(160, 40, &view);

    assert!(
        frame.contains("none configured"),
        "an unconfigured route must say so:\n{frame}"
    );
    for invented in ["claude-3-5-sonnet", "gpt-4o", "Local Directory"] {
        assert!(
            !frame.contains(invented),
            "launcher invented {invented}:\n{frame}"
        );
    }
}

#[test]
fn the_launcher_shows_the_policy_the_runtime_is_actually_enforcing() {
    let mut view = view_with_messages(vec![]);
    view.snapshot.policy = smed::core::policy::PolicyMode::ReadOnly;

    let frame = render_at(160, 40, &view);
    assert!(frame.contains("IN EFFECT"), "frame:\n{frame}");

    // The marker sits on the card whose mode the runtime reports, so a reader
    // cannot mistake the cursor position for the live policy.
    let marked = frame
        .lines()
        .find(|line| line.contains("IN EFFECT"))
        .unwrap_or_default();
    assert!(marked.contains("Research"), "frame:\n{frame}");
}

/// Each preset's prose is checked against the decision table it describes.
///
/// Lives here rather than beside the presets because `tui` may not import
/// `policy` (`AGENTS.md` §2.1); an integration test sees both.
#[test]
fn launcher_preset_copy_matches_the_policy_decision_table() {
    use smed::core::policy::PolicyMode;
    use smed::policy::{PolicyDecision, decide};

    for preset in smed::tui::launcher::PRESETS {
        // Every preset's copy opens by promising free reads.
        assert_eq!(
            decide(preset.mode, ToolTier::Read, false),
            PolicyDecision::Allow,
            "{} promises free reads",
            preset.name
        );
        assert!(
            !preset.mode.is_full_auto(),
            "full-auto is reachable only through the armed grant"
        );
    }

    assert_eq!(
        decide(PolicyMode::ReadOnly, ToolTier::Write, false),
        PolicyDecision::Deny(ReasonCode::PolicyReadOnly),
        "Research claims writes are refused outright"
    );
    assert_eq!(
        decide(PolicyMode::Ask, ToolTier::Write, false),
        PolicyDecision::Ask,
        "Governed claims writes stop for approval"
    );
    assert_eq!(
        decide(PolicyMode::WorkspaceWrite, ToolTier::Write, false),
        PolicyDecision::Allow,
        "Workspace Write claims writes proceed"
    );
    assert_eq!(
        decide(PolicyMode::WorkspaceWrite, ToolTier::Execute, false),
        PolicyDecision::Ask,
        "Workspace Write claims commands still stop for approval"
    );
}

#[test]
fn the_empty_dashboard_summarises_provider_readiness_without_listing_names() {
    use smed::core::runtime::{ProviderConnection, ProviderConnectionState};

    let connection = |provider: &str, state| ProviderConnection {
        provider: ProviderId::new(provider),
        state,
        detail: None,
    };
    let mut view = view_with_messages(vec![]);
    view.snapshot.providers = Arc::new(vec![
        connection("openai", ProviderConnectionState::Connected),
        connection("gemini", ProviderConnectionState::Disconnected),
    ]);

    let frame = render_at(100, 30, &view);
    assert!(!frame.contains("openai"), "frame:\n{frame}");
    assert!(!frame.contains("forge"), "frame:\n{frame}");
    assert!(!frame.contains("gemini"), "frame:\n{frame}");
}

#[test]
fn help_overlay_documents_only_live_shortcuts() {
    let mut view = view_with_messages(vec![]);
    view.overlay = Overlay::Help;

    let frame = render_at(100, 30, &view);
    assert!(frame.contains(" KEYMAP "), "frame:\n{frame}");
    assert!(frame.contains("CTRL-C"), "frame:\n{frame}");
    assert!(frame.contains("SHIFT-TAB"), "frame:\n{frame}");
    assert!(frame.contains("CTRL-O"), "frame:\n{frame}");
    assert!(frame.contains("F1"), "frame:\n{frame}");
    assert!(frame.contains("CMD-V/C"), "frame:\n{frame}");
    assert!(frame.contains("CTRL-V/Y"), "frame:\n{frame}");
    // Commands come from the one registry now, so they appear individually
    // rather than grouped into hand-written rows.
    assert!(frame.contains("/help"), "frame:\n{frame}");
    assert!(frame.contains("/skills"), "frame:\n{frame}");
    assert!(frame.contains("/usage"), "frame:\n{frame}");
    assert!(frame.contains("/policy"), "frame:\n{frame}");
    assert!(frame.contains("/auth"), "frame:\n{frame}");
    // Routing is only usable if it is discoverable: the registry-driven panel
    // must surface the route/role selectors alongside the rest.
    assert!(frame.contains("/route"), "frame:\n{frame}");
    assert!(frame.contains("/role"), "frame:\n{frame}");
}

#[test]
fn skills_overlay_distinguishes_available_active_and_invalid_entries() {
    let mut view = view_with_messages(vec![]);
    view.snapshot.skills = Arc::new(vec![
        SkillSummary {
            name: "guarded-review".to_owned(),
            description: "Use for guarded reviews".to_owned(),
            location: "/tmp/demo-project/.agents/skills/guarded-review/SKILL.md".to_owned(),
            scope: SkillScope::Project,
        },
        SkillSummary {
            name: "docs".to_owned(),
            description: "Use for documentation".to_owned(),
            location: "/tmp/user/.agents/skills/docs/SKILL.md".to_owned(),
            scope: SkillScope::User,
        },
    ]);
    view.snapshot.activated_skills = Arc::new(vec!["guarded-review".to_owned()]);
    view.snapshot.workspace_trusted = true;
    view.snapshot.context_diagnostics = Arc::new(vec![ContextDiagnostic {
        code: ReasonCode::SchemaInvalid,
        detail: "ignored malformed skill".to_owned(),
    }]);
    view.overlay = Overlay::Skills;

    let frame = render_at(100, 28, &view);
    assert!(frame.contains(" SKILLS "));
    assert!(frame.contains("ACTIVE") && frame.contains("guarded-review [project]"));
    assert!(frame.contains("AVAILABLE") && frame.contains("docs [user]"));
    assert!(frame.contains("SCHEMA_INVALID"));
    assert!(frame.contains("project trust granted"));
}

#[test]
fn model_switch_notice_discloses_the_provider_private_state_boundary() {
    let mut view = view_with_messages(vec![]);
    view.apply(&SmedEvent::ModelChanged {
        session: SessionId::new(),
        provider: ProviderId::new("anthropic"),
        model: ModelId::new("claude-sonnet-5"),
    });

    let frame = render_at(110, 24, &view);
    assert!(frame.contains("MODEL CHANGED"), "frame:\n{frame}");
    assert!(
        frame.contains("anthropic:claude-sonnet-5"),
        "frame:\n{frame}"
    );
    // Asserted in fragments rather than as one contiguous run: the navigation
    // shell's work rail narrows the transcript, so the disclosure now wraps
    // mid-sentence. It is still shown in full — what must not regress is the
    // claim, not the column it happens to break at.
    assert!(
        frame.contains("provider-private reasoning"),
        "frame:\n{frame}"
    );
    assert!(frame.contains("not migrated"), "frame:\n{frame}");
}

#[test]
fn multiline_composer_follows_its_newest_visible_line() {
    let mut view = view_with_messages(vec![]);
    view.composer = "first\nsecond\nthird\nfourth".to_owned();
    view.composer_cursor = view.composer.chars().count();

    let frame = render_at(80, 20, &view);
    assert!(
        !frame.contains("first"),
        "old composer line remained visible:\n{frame}"
    );
    assert!(frame.contains("second"), "frame:\n{frame}");
    assert!(frame.contains("third"), "frame:\n{frame}");
    assert!(frame.contains("fourth"), "frame:\n{frame}");
}

#[test]
fn a_pasted_image_shows_a_caption_and_never_the_bare_path() {
    // The link is replaced by what it depicts. On a terminal with no graphics
    // protocol — every frame test, and any plain TTY — that is a caption plus
    // the reason, never a raw `file://` line masquerading as prose.
    let view = view_with_messages(vec![CanonicalMessage::user(
        "look at this ![pasted_image](file:///tmp/demo-project/.smed/assets/paste_1.png)",
    )]);

    let frame = render_at(80, 24, &view);
    assert!(frame.contains("look at this"));
    assert!(frame.contains("pasted_image"));
    assert!(
        !frame.contains("file://") && !frame.contains("paste_1.png"),
        "the transcript must not print the link target it replaced:\n{frame}"
    );
    assert!(
        frame.contains("image rendering unavailable"),
        "a link that cannot be drawn says so rather than vanishing:\n{frame}"
    );
}

#[test]
fn an_image_outside_the_workspace_is_refused_by_name() {
    // Containment is rechecked in the TUI because `tui` cannot import `policy`.
    // A message naming a path outside the workspace gets a refusal, not pixels.
    let mut view = view_with_messages(vec![CanonicalMessage::user("![secret](file:///etc/hosts)")]);
    view.snapshot.workspace_root = Some(std::path::PathBuf::from("/tmp/demo-project"));

    let frame = render_at(80, 24, &view);
    assert!(
        !frame.contains("/etc/hosts"),
        "a refused path must not be echoed into the transcript:\n{frame}"
    );
    assert!(frame.contains("secret"));
}

#[test]
fn timeline_shows_user_and_assistant_turns() {
    let view = view_with_messages(vec![
        CanonicalMessage::user("what does this repo do?"),
        CanonicalMessage::assistant(
            vec![ContentBlock::Text {
                text: "It is a coding harness.".to_owned(),
            }],
            ProviderId::new("fake"),
            ModelId::new("fake-1"),
        ),
    ]);

    let frame = render_at(80, 24, &view);
    assert!(frame.contains("You"));
    assert!(frame.contains("fake-1"));
    assert!(frame.contains("what does this repo do?"));
    assert!(frame.contains("It is a coding harness."));
}

#[test]
fn a_tool_call_renders_as_a_proposal_not_as_an_action() {
    // Showing a proposed call as though it ran would be a lie about state.
    let view = view_with_messages(vec![CanonicalMessage::assistant(
        vec![ContentBlock::ToolCall(ToolCall {
            id: "call_1".to_owned(),
            name: "read_file".to_owned(),
            arguments: serde_json::json!({ "path": "src/lib.rs" }),
            provider_signature: None,
        })],
        ProviderId::new("fake"),
        ModelId::new("fake-1"),
    )]);

    let frame = render_at(80, 20, &view);
    assert!(
        frame.contains("◦ read_file"),
        "call lifecycle missing:\n{frame}"
    );
    assert!(
        frame.contains("no outcome recorded") && !frame.contains("✓ read_file"),
        "an unexecuted call must not look successful:\n{frame}"
    );
}

#[test]
fn a_tool_result_renders_its_stable_outcome() {
    let view = view_with_messages(vec![CanonicalMessage::tool_result(
        "call_1",
        "edit_file",
        ToolResult::refused(ReasonCode::StaleFileVersion, "read it again"),
    )]);

    let frame = render_at(80, 20, &view);
    assert!(frame.contains("✗ edit_file  — STALE_FILE_VERSION"));
    assert!(frame.contains("read it again"));
}

#[test]
fn an_armed_envelope_states_what_it_authorises_before_confirming() {
    use smed::core::envelope::SpawnEnvelope;

    let mut view = view_with_messages(vec![]);
    view.envelope_armed = Some(Box::new(SpawnEnvelope {
        ceiling: smed::core::policy::PolicyMode::ReadOnly,
        max_children: 32,
        max_per_call: 8,
        max_provider_turns: 120,
        routes: Vec::new(),
        expires_after_turns: 20,
    }));

    let frame = render_at(140, 20, &view);
    // Everything a human needs to weigh the grant: how many, how wide, how
    // long, and that the spawns will not be asked about individually.
    assert!(
        frame.contains("32"),
        "the child count must be shown:\n{frame}"
    );
    assert!(
        frame.contains("read-only"),
        "the ceiling must be shown:\n{frame}"
    );
    assert!(
        frame.contains("without asking"),
        "it must say the spawns will not be prompted:\n{frame}"
    );
    assert!(
        frame.contains("[y] confirm"),
        "arming must cost a deliberate keystroke:\n{frame}"
    );
}

#[test]
fn a_live_envelope_shows_what_remains_in_the_header() {
    use smed::core::envelope::{ActiveEnvelope, SpawnEnvelope};

    let mut view = view_with_messages(vec![]);
    let mut active = ActiveEnvelope::new(SpawnEnvelope {
        ceiling: smed::core::policy::PolicyMode::ReadOnly,
        max_children: 32,
        max_per_call: 8,
        max_provider_turns: 120,
        routes: Vec::new(),
        expires_after_turns: 20,
    });
    active.draw(8, 30);
    view.snapshot.envelope = Some(active);

    let frame = render_at(140, 20, &view);
    assert!(
        frame.contains("ENVELOPE") && frame.contains("24/32"),
        "a standing authorisation must be visible without running a command:\n{frame}"
    );
}

#[test]
fn a_run_that_answers_only_in_finish_task_still_shows_its_answer() {
    // Observed with gemini-3.5-flash-low: asked what it could do, the model
    // emitted no assistant prose at all — one list_files, then finish_task
    // carrying the whole answer. The completion rendered as a bare tool row and
    // the user saw a file listing and nothing else.
    let mut view = view_with_messages(vec![CanonicalMessage::tool_result(
        "call_1",
        "finish_task",
        ToolResult::ok("I can read, search, and edit files in this repository.").with_effect(
            ToolEffect::Completion {
                outcome: "verified".to_owned(),
            },
        ),
    )]);
    view.show_tool_details = false;

    let frame = render_at(80, 20, &view);
    assert!(
        frame.contains("I can read, search, and edit files"),
        "the summary a run ended on is the answer, not a detail:\n{frame}"
    );
}

#[test]
fn a_completion_summary_is_not_printed_twice_when_details_are_open() {
    let mut view = view_with_messages(vec![CanonicalMessage::tool_result(
        "call_1",
        "finish_task",
        ToolResult::ok("done and verified").with_effect(ToolEffect::Completion {
            outcome: "verified".to_owned(),
        }),
    )]);
    view.show_tool_details = true;

    let frame = render_at(80, 20, &view);
    assert_eq!(
        frame.matches("done and verified").count(),
        1,
        "the outcome line and the detail body must not both render it:\n{frame}"
    );
}

#[test]
fn tool_details_can_collapse_without_hiding_the_outcome() {
    let mut view = view_with_messages(vec![CanonicalMessage::tool_result(
        "call_1",
        "edit_file",
        ToolResult::refused(ReasonCode::StaleFileVersion, "private detail"),
    )]);
    view.show_tool_details = false;

    let frame = render_at(80, 20, &view);
    assert!(frame.contains("✗ edit_file  — STALE_FILE_VERSION"));
    assert!(!frame.contains("private detail"));
}

#[test]
fn approval_modal_names_the_boundary_and_exact_action() {
    let mut view = view_with_messages(vec![]);
    view.snapshot.pending_approval = Some(PendingApproval {
        id: ApprovalId::new(),
        tool_name: "run_command".to_owned(),
        tier: ToolTier::Execute,
        preview: "cargo test --all-features".to_owned(),
    });

    let frame = render_at(100, 24, &view);
    assert!(frame.contains("AUTHORIZATION GATE"), "frame:\n{frame}");
    assert!(
        frame.contains("cargo test --all-features"),
        "frame:\n{frame}"
    );
    assert!(
        frame.contains("not an OS security sandbox"),
        "frame:\n{frame}"
    );
    assert!(frame.contains("exact command"), "frame:\n{frame}");
}

#[test]
fn streaming_text_is_visible_before_the_run_finishes() {
    let mut view = view_with_messages(vec![]);
    let session = SessionId::new();
    let run = RunId::new();

    view.apply(&SmedEvent::RunStarted { session, run });
    view.apply(&SmedEvent::TextDelta {
        session,
        run,
        text: "partial answer".to_owned(),
    });

    let frame = render_at(80, 20, &view);
    assert!(frame.contains("partial answer"));
    assert!(
        frame.to_ascii_lowercase().contains("live"),
        "status must show the run is live:\n{frame}"
    );
}

#[test]
fn a_finished_run_never_labels_incomplete_as_done() {
    let session = SessionId::new();
    let run = RunId::new();

    let mut done = view_with_messages(vec![]);
    done.apply(&SmedEvent::RunFinished {
        session,
        run,
        reason: FinishReason::Stop,
    });
    assert!(render_at(80, 20, &done).contains("done"));

    let mut incomplete = view_with_messages(vec![]);
    incomplete.apply(&SmedEvent::RunFinished {
        session,
        run,
        reason: FinishReason::Incomplete,
    });
    let frame = render_at(80, 20, &incomplete);
    assert!(
        frame.contains("incomplete"),
        "a truncated response must not read as success:\n{frame}"
    );
}

#[test]
fn a_failure_shows_its_stable_reason_code() {
    let mut view = view_with_messages(vec![]);
    view.apply(&SmedEvent::RunFailed {
        session: SessionId::new(),
        run: RunId::new(),
        code: smed::core::error::ReasonCode::ProviderRateLimit,
        detail: "slow down".to_owned(),
    });

    let frame = render_at(80, 20, &view);
    assert!(frame.contains("PROVIDER_RATE_LIMIT"), "frame:\n{frame}");
    assert_eq!(
        frame.matches("slow down").count(),
        1,
        "the full explanation belongs in the transcript, not duplicated in the footer:\n{frame}"
    );
    assert!(frame.contains("see transcript"), "frame:\n{frame}");
}

#[test]
fn subscription_quota_shows_its_typed_code_and_reset() {
    let mut view = view_with_messages(vec![]);
    view.apply(&SmedEvent::RunFailed {
        session: SessionId::new(),
        run: RunId::new(),
        code: smed::core::error::ReasonCode::ProviderPlanQuota,
        detail: "subscription plan quota exhausted; reset at Some(1700000000)".to_owned(),
    });

    let frame = render_at(100, 20, &view);
    assert!(frame.contains("PROVIDER_PLAN_QUOTA"), "frame:\n{frame}");
    assert!(frame.contains("1700000000"), "frame:\n{frame}");
}

#[test]
fn usage_overlay_reports_received_windows_and_never_invents_absent_data() {
    let mut absent = view_with_messages(vec![]);
    absent.overlay = Overlay::Usage;
    let frame = render_at(100, 24, &absent);
    assert!(
        frame.to_lowercase().contains("no quota data reported"),
        "frame:\n{frame}"
    );
    assert!(
        frame.to_lowercase().contains("will not guess"),
        "frame:\n{frame}"
    );

    let mut reported = view_with_messages(vec![]);
    reported.quota = Some(QuotaSnapshot {
        provider: ProviderId::new("openai-codex"),
        windows: vec![QuotaWindow {
            label: "primary window".to_owned(),
            used_fraction: 0.91,
            resets_at: Some(time::OffsetDateTime::now_utc() + time::Duration::minutes(12)),
        }],
    });
    reported.overlay = Overlay::Usage;
    let frame = render_at(110, 26, &reported);
    assert!(
        frame.to_lowercase().contains("openai-codex")
            && frame
                .to_lowercase()
                .contains("reported on the recent api response"),
        "frame:\n{frame}"
    );
    assert!(frame.contains("91% used"), "frame:\n{frame}");
    assert!(frame.contains("resets in"), "frame:\n{frame}");
}

#[test]
fn configured_quota_is_labelled_as_an_estimate() {
    let mut view = view_with_messages(vec![]);
    view.snapshot.quota_reserve = smed::core::continuation::QuotaReserveStatus {
        basis: smed::core::continuation::QuotaReserveBasis::ConfiguredTokens { limit: 100_000 },
        used_fraction: Some(0.81),
        ..smed::core::continuation::QuotaReserveStatus::default()
    };
    view.overlay = Overlay::Usage;
    let frame = render_at(100, 24, &view);
    assert!(
        frame.to_lowercase().contains("configured token budget"),
        "frame:\n{frame}"
    );
    assert!(frame.contains("estimate"), "frame:\n{frame}");
}

#[test]
fn resume_advisor_has_no_enter_default_and_labels_its_estimate() {
    let mut view = view_with_messages(vec![]);
    view.snapshot.resume_advice = Some(smed::core::continuation::ResumeAdvice {
        warning: smed::core::continuation::ResumeWarning::QuotaStopped {
            resets_at: Some(time::OffsetDateTime::now_utc() + time::Duration::hours(1)),
        },
        estimated_full_resume_tokens: 88_000,
        handoff: Some(smed::core::continuation::HandoffId::new()),
    });
    let frame = render_at(110, 24, &view);
    assert!(frame.contains("RESUME ADVISOR"), "frame:\n{frame}");
    assert!(frame.contains("≈88000 tokens"), "frame:\n{frame}");
    assert!(frame.contains("Enter has no default"), "frame:\n{frame}");
    assert!(frame.contains("[c] compact"), "frame:\n{frame}");
}

#[test]
fn full_auto_and_its_confirmation_are_visually_unmistakable() {
    let mut active = view_with_messages(vec![]);
    active.snapshot.policy = smed::core::policy::PolicyMode::FullAuto;
    active.auto_allowed_side_effects = 3;
    let frame = render_at(140, 24, &active);
    assert!(frame.contains("POLICY full-auto"), "frame:\n{frame}");
    assert!(frame.contains("FULL-AUTO"), "frame:\n{frame}");
    assert!(frame.contains("3 AUTO"), "frame:\n{frame}");

    let mut armed = view_with_messages(vec![]);
    armed.full_auto_armed = true;
    let frame = render_at(140, 24, &armed);
    assert!(frame.contains("FULL-AUTO REQUESTED"), "frame:\n{frame}");
    assert!(frame.contains("[y] confirm"), "frame:\n{frame}");
    assert!(frame.contains("not a sandbox"), "frame:\n{frame}");
}

#[test]
fn live_reasoning_is_sanitized_collapses_and_is_never_present_in_snapshot_history() {
    let mut view = view_with_messages(vec![]);
    let session = SessionId::new();
    let run = RunId::new();
    view.apply(&SmedEvent::RunStarted { session, run });
    view.apply(&SmedEvent::ReasoningDelta {
        session,
        run,
        text: "inspect\x1b[2J files".to_owned(),
    });
    let frame = render_at(90, 22, &view);
    assert!(frame.to_lowercase().contains("thinking"), "frame:\n{frame}");
    assert!(!frame.contains('\x1b'), "escape survived:\n{frame}");

    view.apply(&SmedEvent::TextDelta {
        session,
        run,
        text: "answer".to_owned(),
    });
    let frame = render_at(90, 22, &view);
    assert!(frame.contains("thought for"), "frame:\n{frame}");
    assert!(
        !frame.contains("inspect"),
        "reasoning did not collapse:\n{frame}"
    );
    assert!(view.snapshot.messages.is_empty());
}

#[test]
fn activity_and_failures_name_the_in_flight_intent_at_all_supported_widths() {
    for width in [48, 90, 150] {
        let mut view = view_with_messages(vec![]);
        let session = SessionId::new();
        let run = RunId::new();
        view.apply(&SmedEvent::RunStarted { session, run });
        view.apply(&SmedEvent::ToolAssembling {
            session,
            run,
            name: "list_dir".to_owned(),
        });
        let live = render_at(width, 24, &view);
        assert!(
            live.contains("assembling list_dir"),
            "width {width}:\n{live}"
        );
        assert!(live.contains("turn"), "width {width}:\n{live}");

        view.apply(&SmedEvent::RunFailed {
            session,
            run,
            code: ReasonCode::ProviderProtocol,
            detail: "bad arguments".to_owned(),
        });
        let failed = render_at(width, 24, &view);
        assert!(
            failed.contains("PROVIDER_PROTOCOL"),
            "width {width}:\n{failed}"
        );
        assert!(failed.contains("list_dir"), "width {width}:\n{failed}");
    }
}

#[test]
fn lagging_is_disclosed_rather_than_hidden() {
    let mut view = view_with_messages(vec![]);
    view.note_lagged();

    let frame = render_at(80, 20, &view);
    assert!(
        frame.to_ascii_lowercase().contains("resynced"),
        "a view that dropped events must say so:\n{frame}"
    );
}

#[test]
fn control_characters_from_a_model_cannot_reach_the_screen() {
    // Model output is untrusted input. A raw escape sequence in a message must
    // not be able to repaint the UI or move the cursor.
    let view = view_with_messages(vec![CanonicalMessage::assistant(
        vec![ContentBlock::Text {
            text: "before\x1b[2Jafter".to_owned(),
        }],
        ProviderId::new("fake"),
        ModelId::new("fake-1"),
    )]);

    let frame = render_at(80, 20, &view);
    assert!(
        !frame.contains('\x1b'),
        "an escape sequence reached the frame"
    );
    assert!(frame.contains("before"));
    assert!(frame.contains("after"));
}

#[test]
fn narrow_and_wide_terminals_both_render_without_panicking() {
    let view = view_with_messages(vec![CanonicalMessage::user(
        "a fairly long message that will certainly need to wrap somewhere",
    )]);

    // Wide, normal, and narrow-but-usable.
    for (width, height) in [(200, 50), (80, 24), (30, 10)] {
        let frame = render_at(width, height, &view);
        assert!(!frame.is_empty(), "{width}x{height} rendered nothing");
    }

    // Too small to be honest: say so rather than render something misleading.
    let frame = render_at(12, 4, &view);
    assert!(
        frame.contains("small"),
        "tiny terminal must explain itself:\n{frame}"
    );
}

/// A view halted by interrupted work.
fn view_recovering(kind: smed::core::recovery::InterruptedKind) -> ViewState {
    let mut view = view_with_messages(vec![CanonicalMessage::user("write the file")]);
    let snapshot = RuntimeSnapshot {
        recovery: smed::core::recovery::RecoveryState::Required(
            smed::core::recovery::InterruptedWork {
                run: smed::core::event::RunId::new(),
                kind,
            },
        ),
        ..view.snapshot.clone()
    };
    view.sync(snapshot);
    view
}

fn uncertain_kind() -> smed::core::recovery::InterruptedKind {
    smed::core::recovery::InterruptedKind::EffectUncertain {
        authority: smed::core::recovery::Authority::Policy,
        call: smed::core::message::ToolCall {
            id: "call_1".to_owned(),
            name: "write_file".to_owned(),
            arguments: serde_json::json!({ "path": "a.txt" }),
            provider_signature: None,
        },
        tier: smed::core::tool::ToolTier::Write,
        preview: "+ written".to_owned(),
    }
}

#[test]
fn a_recovery_required_session_states_the_boundary_and_its_choices() {
    let frame = render_at(90, 26, &view_recovering(uncertain_kind()));

    assert!(
        frame.contains("RECOVERY_REQUIRES_DECISION"),
        "the stable reason code must be on screen:\n{frame}"
    );
    assert!(
        frame.contains("EFFECT_UNCERTAIN"),
        "the kind of interruption must be named:\n{frame}"
    );
    assert!(
        frame.contains("write_file"),
        "the human needs to know which tool:\n{frame}"
    );
    assert!(
        frame.contains("[c]") && frame.contains("[e]"),
        "both decisions must be offered:\n{frame}"
    );
    assert!(
        frame.contains("Nothing is retried automatically"),
        "the guarantee must be stated, not assumed:\n{frame}"
    );
}

#[test]
fn an_uncertain_effect_is_never_described_as_done_or_failed() {
    // 's anti-pattern, rendered: "do not infer that an interrupted
    // command failed merely because no completion event exists."
    let frame = render_at(90, 26, &view_recovering(uncertain_kind())).to_lowercase();

    assert!(
        frame.contains("may or may not"),
        "the honest phrasing must survive to the screen:\n{frame}"
    );
    assert!(
        frame.contains("cannot prove"),
        "the boundary must be explicit:\n{frame}"
    );
}

#[test]
fn a_provably_safe_interruption_says_so_rather_than_alarming_the_user() {
    // The other side of honesty: an unapproved proposal provably did not run, and
    // saying "smed cannot tell" there would train users to ignore the warning
    // that matters.
    let frame = render_at(
        90,
        26,
        &view_recovering(smed::core::recovery::InterruptedKind::ProposalUnapproved {
            call: smed::core::message::ToolCall {
                id: "call_1".to_owned(),
                name: "write_file".to_owned(),
                arguments: serde_json::json!({}),
                provider_signature: None,
            },
            tier: smed::core::tool::ToolTier::Write,
            preview: String::new(),
        }),
    );

    assert!(frame.contains("PROPOSAL_UNAPPROVED"));
    assert!(
        frame.contains("can prove this did not run"),
        "a provably safe interruption must be reported as such:\n{frame}"
    );
}

#[test]
fn a_halted_session_shows_a_composer_that_refuses_input() {
    // A directive box that looks ready while the runtime would refuse the
    // directive is a lie the user only discovers after typing.
    let frame = render_at(90, 26, &view_recovering(uncertain_kind()));
    assert!(
        frame.contains("HALTED"),
        "the composer must state its own unavailability:\n{frame}"
    );
}

#[test]
fn a_durable_write_failure_outranks_every_other_status() {
    // Everything below it would be describing a session whose history the
    // database did not accept.
    let mut view = view_with_messages(vec![CanonicalMessage::user("hello")]);
    let snapshot = RuntimeSnapshot {
        store_failure: Some("disk is full".to_owned()),
        ..view.snapshot.clone()
    };
    view.sync(snapshot);

    let frame = render_at(90, 24, &view);
    assert!(
        frame.contains("DURABILITY LOST"),
        "a failed durable write must be visible:\n{frame}"
    );
    assert!(frame.contains("disk is full"));
    assert!(
        frame.contains("HALTED"),
        "a session that cannot persist must not accept more work:\n{frame}"
    );
}

#[test]
fn recovery_frames_stay_legible_at_every_size() {
    let view = view_recovering(uncertain_kind());
    for (width, height) in [(200, 50), (80, 24), (40, 12)] {
        let frame = render_at(width, height, &view);
        assert!(!frame.is_empty(), "{width}x{height} rendered nothing");
        assert!(
            !frame.contains('\x1b'),
            "an escape sequence reached the frame at {width}x{height}"
        );
    }
}

#[test]
fn typing_a_slash_lists_commands_with_their_live_state() {
    let mut view = view_with_messages(vec![]);
    view.composer = "/".to_owned();

    let frame = render_at(100, 26, &view);

    assert!(frame.contains("COMMANDS"), "frame:\n{frame}");
    assert!(frame.contains("/model"), "frame:\n{frame}");
    // The menu doubles as a status readout: the current model is legible
    // without opening the picker.
    assert!(frame.contains("fake/fake-1"), "frame:\n{frame}");
}

#[test]
fn the_command_menu_yields_to_a_waiting_approval() {
    let mut view = view_with_messages(vec![]);
    view.composer = "/mod".to_owned();
    view.snapshot.pending_approval = Some(PendingApproval {
        id: ApprovalId::new(),
        tool_name: "run_command".to_owned(),
        tier: ToolTier::Execute,
        preview: "rm -rf build".to_owned(),
    });

    let frame = render_at(100, 26, &view);

    // A menu covering the question would invite answering something else.
    assert!(!frame.contains("COMMANDS"), "frame:\n{frame}");
}

#[test]
fn auth_overlay_reports_provider_state_without_offering_a_key_field() {
    use smed::core::runtime::{ProviderConnection, ProviderConnectionState};

    let mut view = view_with_messages(vec![]);
    view.snapshot.providers = Arc::new(vec![ProviderConnection {
        provider: ProviderId::new("gemini"),
        state: ProviderConnectionState::Disconnected,
        detail: Some("No credential is registered.".to_owned()),
    }]);
    view.overlay = Overlay::Auth;

    let frame = render_at(100, 26, &view);

    assert!(frame.contains("DISCONNECTED"), "frame:\n{frame}");
    assert!(frame.contains("gemini"), "frame:\n{frame}");
    // The remedy is the CLI, so a secret never enters this transcript.
    assert!(frame.contains("smed auth login"), "frame:\n{frame}");
}

#[test]
fn model_overlay_lists_connected_catalogs_not_disconnected_providers() {
    use smed::core::runtime::{ModelChoice, ProviderConnection, ProviderConnectionState};

    let mut view = view_with_messages(vec![]);
    view.snapshot.providers = Arc::new(vec![
        ProviderConnection {
            provider: ProviderId::new("openai-codex"),
            state: ProviderConnectionState::Connected,
            detail: None,
        },
        ProviderConnection {
            provider: ProviderId::new("gemini"),
            state: ProviderConnectionState::Disconnected,
            detail: Some("No credential is registered.".to_owned()),
        },
    ]);
    view.snapshot.models = Arc::new(vec![ModelChoice {
        descriptor: ModelDescriptor {
            id: ModelId::new("gpt-5.6-sol"),
            provider: ProviderId::new("openai-codex"),
            display_name: "GPT-5.6 Sol".to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: None,
            max_output_tokens: None,
            tier: None,
        },
    }]);
    view.overlay = Overlay::Models;

    let frame = render_at(100, 26, &view);

    assert!(frame.contains("OPENAI-CODEX"), "frame:\n{frame}");
    assert!(frame.contains("gpt-5.6-sol"), "frame:\n{frame}");
    assert!(!frame.contains("gemini"), "frame:\n{frame}");
}

#[test]
fn plan_checklist_renders_at_different_terminal_sizes() {
    use smed::tui::reducer::PlanStep;

    let mut view = view_with_messages(vec![]);
    view.plan_steps = vec![
        PlanStep {
            number: 1,
            description: "First step".to_owned(),
            done: true,
        },
        PlanStep {
            number: 2,
            description: "Second step".to_owned(),
            done: false,
        },
    ];

    // Test sidebar split layout (wide terminal)
    let frame_wide = render_at(120, 25, &view);
    assert!(
        frame_wide.contains("PLAN STEPS"),
        "wide frame:\n{frame_wide}"
    );
    assert!(
        frame_wide.contains("First step"),
        "wide frame:\n{frame_wide}"
    );
    assert!(
        frame_wide.contains("Second step"),
        "wide frame:\n{frame_wide}"
    );

    // Test bottom split layout (narrow but tall terminal)
    let frame_narrow_tall = render_at(80, 30, &view);
    assert!(
        frame_narrow_tall.contains("PLAN STEPS"),
        "narrow tall frame:\n{frame_narrow_tall}"
    );
    assert!(
        frame_narrow_tall.contains("First step"),
        "narrow tall frame:\n{frame_narrow_tall}"
    );
    assert!(
        frame_narrow_tall.contains("Second step"),
        "narrow tall frame:\n{frame_narrow_tall}"
    );
}

#[test]
fn plan_checklist_renders_inline_markdown_instead_of_its_delimiters() {
    use smed::tui::reducer::PlanStep;

    let mut view = view_with_messages(vec![]);
    view.plan_steps = vec![PlanStep {
        number: 1,
        description: "**Read** `AGENTS.md` before editing".to_owned(),
        done: false,
    }];

    let frame = render_at(120, 25, &view);

    assert!(frame.contains("Read AGENTS.md before"), "frame:\n{frame}");
    assert!(frame.contains("editing"), "frame:\n{frame}");
    assert!(!frame.contains("**Read**"), "frame:\n{frame}");
    assert!(!frame.contains("`AGENTS.md`"), "frame:\n{frame}");
}

#[test]
fn session_tree_overlay_renders_properly() {
    let mut view = view_with_messages(vec![]);
    view.overlay = smed::tui::reducer::Overlay::Tree;

    let frame = render_at(120, 25, &view);
    assert!(frame.contains("SESSION TREE"), "frame:\n{frame}");
    assert!(
        frame.contains("No user turns in history yet"),
        "frame:\n{frame}"
    );
}

#[test]
fn view_state_cancelling_dims_timeline_and_shows_overlay() {
    let mut view = view_with_messages(vec![]);
    view.cancelling = true;

    let frame = render_at(120, 25, &view);
    assert!(frame.contains("CANCELLING..."), "frame:\n{frame}");
    assert!(frame.contains("Interrupting active run"), "frame:\n{frame}");
}

#[test]
fn no_whole_terminal_frame_and_no_nested_bordered_regions() {
    let view = view_with_messages(vec![]);
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| layout::render(frame, &view))
        .expect("draw");

    let buffer = terminal.backend().buffer();
    // Edge (0,0) and (79,0) must not be outer box borders
    let top_left = buffer[(0, 0)].symbol();
    let top_right = buffer[(79, 0)].symbol();
    assert_ne!(top_left, "┌", "outer top-left border found");
    assert_ne!(top_right, "┐", "outer top-right border found");

    // Count box border corners in empty state (must be 0 nested box corners)
    let text = render_at(80, 24, &view);
    assert!(
        !text.contains("┌──"),
        "nested ASCII box header found in empty state:\n{text}"
    );
    assert!(
        !text.contains("└──"),
        "nested ASCII box footer found in empty state:\n{text}"
    );
}

#[test]
fn no_color_literals_outside_theme_module() {
    let tui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tui");
    for entry in std::fs::read_dir(tui_dir).expect("read tui dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "rs")
            && path.file_name().unwrap() != "theme.rs"
        {
            let content = std::fs::read_to_string(&path).expect("read file");
            assert!(
                !content.contains("Color::Rgb") && !content.contains("Color::Cyan"),
                "File {} contains raw Color literal construction outside theme.rs",
                path.display()
            );
        }
    }
}

#[test]
fn prose_with_numbered_list_does_not_trigger_proposed_plan_rail() {
    use smed::core::message::{CanonicalMessage, ContentBlock};
    use smed::core::model::{ModelId, ProviderId};

    let msg = CanonicalMessage::assistant(
        vec![ContentBlock::Text {
            text: "# Proposed Plan:\n1. Inspect repository\n2. Run cargo test".to_string(),
        }],
        ProviderId::new("openai"),
        ModelId::new("gpt-4o"),
    );
    let mut view = view_with_messages(vec![msg]);

    // Ensure sync does not auto-populate plan_steps from prose
    view.sync(view.snapshot.clone());
    assert!(
        view.plan_steps.is_empty(),
        "Prose matching numbered list should NOT auto-populate plan_steps"
    );

    let frame = render_at(120, 25, &view);
    assert!(
        !frame.contains("PROPOSED PLAN"),
        "Prose containing numbered list must not render a PROPOSED PLAN rail:\n{frame}"
    );
}

#[test]
fn workspace_types_read_only_projections_derive_from_view_state() {
    let mut view = view_with_messages(vec![]);
    let session = smed::core::event::SessionId::new();
    view.snapshot.session = Some(session);
    view.snapshot.pending_approval = Some(smed::core::policy::PendingApproval {
        id: ApprovalId::new(),
        tool_name: "write_file".to_string(),
        preview: "Write src/main.rs".to_string(),
        tier: smed::core::tool::ToolTier::Write,
    });

    let work_items = view.project_work_items();
    assert_eq!(work_items.len(), 1);
    assert_eq!(
        work_items[0].lifecycle,
        smed::tui::workspace_types::WorkItemLifecycle::NeedsDecision
    );

    let attention_items = view.project_attention_items();
    assert_eq!(attention_items.len(), 1);
    assert_eq!(
        attention_items[0].priority,
        smed::tui::workspace_types::AttentionPriority::ApprovalRequired
    );
    assert_eq!(attention_items[0].reason_code, "TOOL_APPROVAL_REQUIRED");
}

#[test]
fn workspace_shell_renders_responsive_layouts_at_wide_medium_narrow_viewports() {
    let view = view_with_messages(vec![]);

    let wide_frame = render_at(160, 40, &view);
    assert!(!wide_frame.is_empty(), "Wide frame renders cleanly");

    let wide_layout = smed::tui::shell::compute_shell_layout_with_context(
        ratatui::layout::Rect::new(0, 0, 160, 40),
        false,
        true,
    );
    assert_eq!(wide_layout.tier, smed::tui::shell::TerminalWidthTier::Wide);
    assert!(wide_layout.left_rail.is_some());

    let med_layout = smed::tui::shell::compute_shell_layout_with_context(
        ratatui::layout::Rect::new(0, 0, 100, 30),
        false,
        true,
    );
    assert_eq!(med_layout.tier, smed::tui::shell::TerminalWidthTier::Medium);
    assert!(med_layout.left_rail.is_some());

    let narrow_layout =
        smed::tui::shell::compute_shell_layout(ratatui::layout::Rect::new(0, 0, 70, 20));
    assert_eq!(
        narrow_layout.tier,
        smed::tui::shell::TerminalWidthTier::Narrow
    );
    assert!(narrow_layout.left_rail.is_none());
}

#[test]
fn jump_palette_modal_overlay_renders_on_ctrl_j_active() {
    let mut view = view_with_messages(vec![]);
    view.jump_state.active = true;
    view.jump_state.query = "config".to_string();

    let items = smed::tui::jump_palette::build_jump_items(&view);
    let filtered = smed::tui::jump_palette::filter_jump_items(&items, &view.jump_state.query);
    assert!(!items.is_empty(), "Jump items index should be non-empty");
    assert!(
        !filtered.is_empty(),
        "Filtered jump items for 'config' should match command"
    );
}

#[test]
fn viewport_scroll_engine_pins_and_unpins_history() {
    let mut state = smed::tui::viewport::ViewportState::new();
    assert_eq!(
        state.intent,
        smed::tui::viewport::ViewportIntent::FollowOutput
    );
    assert!(!state.is_pinned());

    state.set_bounds(100, 20);
    state.scroll_up(5);
    assert!(state.is_pinned());
    assert_eq!(state.current_offset(), 5);

    state.end();
    assert_eq!(
        state.intent,
        smed::tui::viewport::ViewportIntent::FollowOutput
    );
    assert!(!state.is_pinned());
}

#[test]
fn attention_queue_priority_sorting_and_navigation() {
    let mut queue = smed::tui::workspace_types::AttentionQueue::new();
    assert_eq!(queue.selected_index, 0);

    queue.move_cursor_down(3);
    assert_eq!(queue.selected_index, 1);

    queue.move_cursor_down(3);
    assert_eq!(queue.selected_index, 2);

    queue.move_cursor_down(3);
    assert_eq!(queue.selected_index, 0);

    queue.move_cursor_up(3);
    assert_eq!(queue.selected_index, 2);
}

#[test]
fn workspace_surface_tab_cycling() {
    use smed::tui::workspace_types::WorkspaceSurface;
    let surface = WorkspaceSurface::Work;
    assert_eq!(surface.next(), WorkspaceSurface::Conversation);
    assert_eq!(surface.next().next(), WorkspaceSurface::Plan);
    assert_eq!(surface.previous(), WorkspaceSurface::Attention);
}

#[test]
fn structured_plan_surface_renders_step_list_and_details() {
    let mut view = view_with_messages(vec![]);
    view.active_surface = smed::tui::workspace_types::WorkspaceSurface::Plan;

    let frame = render_at(120, 30, &view);
    assert!(!frame.is_empty(), "Plan surface renders without crashing");
}

#[test]
fn changes_unified_diff_surface_renders_file_tree_and_diffs() {
    let mut view = view_with_messages(vec![]);
    view.active_surface = smed::tui::workspace_types::WorkspaceSurface::Changes;

    let frame = render_at(120, 30, &view);
    assert!(
        !frame.is_empty(),
        "Changes surface renders without crashing"
    );
}

#[test]
fn verification_evidence_surface_renders_telemetry_and_log_cards() {
    let mut view = view_with_messages(vec![]);
    view.active_surface = smed::tui::workspace_types::WorkspaceSurface::Verify;

    let frame = render_at(120, 30, &view);
    assert!(!frame.is_empty(), "Verify surface renders without crashing");
}

#[test]
fn quick_launcher_dashboard_renders() {
    let view = view_with_messages(vec![]);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30))
        .expect("Terminal test backend creation should succeed");
    terminal
        .draw(|frame| {
            smed::tui::launcher::render_quick_launcher(frame, frame.area(), &view, view.launcher);
        })
        .expect("Draw quick launcher frame should succeed");
    assert!(!terminal.backend().buffer().content().is_empty());
}
