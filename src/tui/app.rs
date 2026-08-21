//! The TUI event loop.
//!
//! A client of [`MjolnrRuntime`] and nothing more: it renders snapshots, reduces
//! events, and sends commands. It never calls a provider, never touches the
//! store, and never owns the transcript.
//!
//! The loop multiplexes terminal input, runtime events, and a redraw tick, all
//! bounded. Two lessons from the Phase 0 spike are structural here rather than
//! remembered: a `select!` arm that completes immediately is disabled, and
//! teardown lives outside the loop so an early `?` cannot skip it.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::broadcast::error::RecvError;

use crate::core::command::MjolnrCommand;
use crate::core::model::{ModelId, ProviderId};
use crate::core::runtime::MjolnrRuntime;
use crate::tui::keymap::{InputAction, InputContext};
use crate::tui::layout;
use crate::tui::reducer::ViewState;

/// Redraw cadence. Renders are driven by a dirty flag plus this tick, never one
/// render per delta (AGENTS.md §5).
const RENDER_TICK: Duration = Duration::from_millis(33);

/// What the loop decided to do with an input event.
enum Flow {
    Continue,
    Redraw,
    /// The alternate screen was left and re-entered (editor, auth): ratatui's
    /// back buffer is stale, so the next draw must repaint every cell or the
    /// screen keeps fragments of the suspended session.
    HardRedraw,
    Quit,
}

/// Interactive OAuth logins, injected by the composition root (/// the TUI may never call a provider). The TUI suspends the terminal and
/// delegates; implementations may print and read stdin freely while it is
/// suspended, and must write the credential to the credential store themselves.
#[async_trait::async_trait]
pub trait AuthFlows: Send + Sync {
    /// Run the interactive OAuth login for `provider` on a plain terminal.
    /// Returns the access-token expiry (Unix time), or a rendered error.
    async fn oauth_login(&self, provider: &str) -> Result<i64, String>;

    /// Validate and persist LM Studio's non-secret project endpoint.
    ///
    /// The composition root owns provider configuration; the TUI only gathers
    /// the address while its terminal is suspended.
    fn configure_lm_studio_endpoint(&self, address: &str) -> Result<String, String>;

    /// Clear a stored LM Studio token for explicit keyless operation. Returns
    /// whether an environment token still overrides that state.
    fn clear_lm_studio_token(&self) -> Result<bool, String>;
}

/// Ask the terminal which graphics protocol and font size it has.
///
/// Called after the alternate screen is entered and *before* `EventStream`
/// starts consuming stdin: the terminal's replies are ordinary input, and a
/// reader already running would eat them. A terminal that answers nothing
/// yields half-blocks rather than an error, so this never blocks startup.
fn detect_image_protocol(view: &ViewState) {
    if let Ok(picker) = ratatui_image::picker::Picker::from_query_stdio()
        && let Ok(mut store) = view.images.try_borrow_mut()
    {
        store.enable(picker);
    }
}

/// Run the TUI until the user quits.
pub async fn run(
    terminal: &mut DefaultTerminal,
    runtime: &dyn MjolnrRuntime,
    auth: &dyn AuthFlows,
) -> io::Result<()> {
    initialize_theme();
    let mut events = runtime.subscribe();
    // Subscribed *before* the first read, so a state change between the two is
    // delivered rather than missed.
    let mut snapshots = runtime.snapshots();
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(RENDER_TICK);

    let mut view = ViewState::default();
    detect_image_protocol(&view);
    view.sync(runtime.snapshot());

    let mut dirty = true;

    loop {
        if dirty {
            terminal.draw(|frame| layout::render(frame, &view))?;
            dirty = false;
        }

        tokio::select! {
            maybe_input = input.next() => {
                match maybe_input {
                    Some(Ok(event)) => match handle_input(&event, &mut view, runtime, auth).await {
                        Flow::Quit => break,
                        Flow::Redraw => dirty = true,
                        Flow::HardRedraw => {
                            let _ = terminal.clear();
                            dirty = true;
                        }
                        Flow::Continue => {}
                    },
                    Some(Err(error)) => return Err(error),
                    None => break,
                }
            }
            // State, as it becomes true. This arm is why a resumed session
            // renders: it restores a transcript and, when nothing was
            // interrupted, announces nothing.
            changed = snapshots.changed() => {
                match changed {
                    Ok(snapshot) => {
                        handle_snapshot_update(snapshot, &mut view, runtime).await;
                        dirty = true;
                    }
                    // The runtime is gone; the event feed will say so too.
                    Err(_) => break,
                }
            }
            incoming = events.recv() => {
                match incoming {
                    Ok(event) => {
                        // Only the in-flight stream is reduced here. Everything
                        // durable arrives as state on the snapshot arm above.
                        view.apply(&event);
                        dirty = true;
                    }
                    // The feed is bounded. Falling behind costs render deltas,
                    // never memory — resync and tell the user rather than
                    // showing a silently incomplete timeline.
                    Err(RecvError::Lagged(_)) => {
                        view.note_lagged();
                        view.sync(runtime.snapshot());
                        dirty = true;
                    }
                    Err(RecvError::Closed) => break,
                }
            }
            _ = tick.tick() => {
                if view.keymap.expire(Instant::now()) {
                    dirty = true;
                }
                if view.animating() {
                    view.advance_tick();
                    dirty = true;
                }
            }
        }
    }

    Ok(())
}

async fn handle_snapshot_update(
    snapshot: crate::core::runtime::RuntimeSnapshot,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
) {
    let was_active = view.snapshot.run_active;
    view.sync(snapshot);
    if was_active && !view.snapshot.run_active && !view.composer_queue.is_empty() {
        let next_prompt = view.composer_queue.remove(0);
        view.composer = next_prompt;
        view.composer_cursor = view.composer.chars().count();
        let _ = submit(view, runtime).await;
    }
}

/// Resolve a grant waiting on its one-key confirmation.
///
/// Full-auto and the spawn envelope share this ceremony because they are the
/// same kind of act: authority over work that has not been proposed yet. Any key
/// but `y` withdraws, so the expensive answer is never the reflexive one.
///
/// `None` means nothing was armed and ordinary input handling continues.
async fn resolve_armed_grant(
    key: crossterm::event::KeyEvent,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
) -> Option<Flow> {
    let confirmed = key.code == crossterm::event::KeyCode::Char('y');
    if view.full_auto_armed {
        view.full_auto_armed = false;
        if confirmed {
            let _ = runtime
                .dispatch(MjolnrCommand::SetPolicy {
                    mode: crate::core::policy::PolicyMode::FullAuto,
                })
                .await;
        }
        return Some(Flow::Redraw);
    }
    if let Some(envelope) = view.envelope_armed.take() {
        if confirmed {
            let _ = runtime
                .dispatch(MjolnrCommand::ArmSpawnEnvelope { envelope })
                .await;
        }
        return Some(Flow::Redraw);
    }
    None
}

async fn handle_input(
    event: &Event,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
    auth: &dyn AuthFlows,
) -> Flow {
    let key = match event {
        Event::Resize(_, _) => return Flow::Redraw,
        Event::Paste(text)
            if view.snapshot.pending_approval.is_none()
                && view.overlay == crate::tui::reducer::Overlay::None
                && !view.snapshot.recovery.is_required() =>
        {
            view.keymap.disarm();
            view.append_composer(text);
            return Flow::Redraw;
        }
        Event::Key(key) => key,
        // The wheel is what a person reaches for to look back through a
        // transcript; PageUp is the thing they find only after giving up.
        Event::Mouse(mouse) => {
            return match mouse.kind {
                crossterm::event::MouseEventKind::ScrollUp => {
                    view.scroll_up();
                    Flow::Redraw
                }
                crossterm::event::MouseEventKind::ScrollDown => {
                    view.scroll_down();
                    Flow::Redraw
                }
                crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                    handle_mouse_left_click(*mouse, view)
                }
                _ => Flow::Continue,
            };
        }
        Event::FocusGained | Event::FocusLost | Event::Paste(_) => {
            return Flow::Continue;
        }
    };

    // `KeyEventKind::Press` is load-bearing: crossterm's default mode only
    // delivers Press, so omitting it looks fine until the kitty keyboard
    // protocol is enabled and a key *release* starts acting on its own.
    if key.kind != KeyEventKind::Press {
        return Flow::Continue;
    }

    if let Some(flow) = handle_direct_key_intercepts(*key, view, runtime).await {
        return flow;
    }

    let context = build_input_context(view);
    let action = view.keymap.resolve(*key, context, Instant::now());
    apply_action(action, view, runtime, auth).await
}

async fn handle_direct_key_intercepts(
    key: crossterm::event::KeyEvent,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
) -> Option<Flow> {
    if let Some(flow) = resolve_armed_grant(key, view, runtime).await {
        return Some(flow);
    }
    if let Some(flow) = apply_jump_palette_key(key, view) {
        return Some(flow);
    }
    if let Some(flow) = apply_launcher_key(key, view, runtime).await {
        return Some(flow);
    }
    if view.snapshot.resume_advice.is_some() && !view.snapshot.recovery.is_required() {
        if let Some(choice) = resume_choice_for(key.code) {
            let _ = runtime
                .dispatch(MjolnrCommand::ResolveResume { choice })
                .await;
            return Some(Flow::Redraw);
        }
        return Some(Flow::Continue);
    }
    if (key.code == crossterm::event::KeyCode::Char('p')
        || key.code == crossterm::event::KeyCode::Char('P'))
        && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
    {
        view.toggle_auxiliary_panel();
        return Some(Flow::Redraw);
    }
    if key.code == crossterm::event::KeyCode::Esc
        && view.overlay == crate::tui::reducer::Overlay::None
        && view.auxiliary_panel_visible
    {
        view.hide_auxiliary_panel();
        return Some(Flow::Redraw);
    }
    if key.code == crossterm::event::KeyCode::Tab
        && !key
            .modifiers
            .contains(crossterm::event::KeyModifiers::SHIFT)
        && view.overlay == crate::tui::reducer::Overlay::None
        && !view.snapshot.run_active
        && view.fleet_visible()
        && !crate::tui::commands::menu_applies(&view.composer)
    {
        view.cycle_fleet_focus();
        return Some(Flow::Redraw);
    }
    None
}

fn build_input_context(view: &ViewState) -> InputContext {
    let plan_approval_pending = view.snapshot.plan.as_ref().is_some_and(|workflow| {
        matches!(
            workflow.stage,
            crate::core::plan::PlanStage::Proposed { .. }
                | crate::core::plan::PlanStage::Reviewed { .. }
        )
    });
    InputContext {
        run_active: view.snapshot.run_active,
        composer_empty: view.composer.is_empty(),
        help_open: view.overlay == crate::tui::reducer::Overlay::Help,
        approval_tier: view
            .snapshot
            .pending_approval
            .as_ref()
            .map(|pending| pending.tier),
        recovery_required: view.snapshot.recovery.is_required(),
        picker_open: view.overlay == crate::tui::reducer::Overlay::Models
            || view.overlay == crate::tui::reducer::Overlay::Tree
            || view.overlay == crate::tui::reducer::Overlay::Auth
            || view.overlay == crate::tui::reducer::Overlay::Theme
            || view.overlay == crate::tui::reducer::Overlay::Config
            || view.overlay == crate::tui::reducer::Overlay::Discovery,
        command_menu_open: crate::tui::commands::menu_applies(&view.composer)
            && view.overlay == crate::tui::reducer::Overlay::None,
        plan_approval_pending,
    }
}

fn resume_choice_for(
    code: crossterm::event::KeyCode,
) -> Option<crate::core::continuation::ResumeChoice> {
    match code {
        crossterm::event::KeyCode::Char('c') => {
            Some(crate::core::continuation::ResumeChoice::Compact)
        }
        crossterm::event::KeyCode::Char('n') => {
            Some(crate::core::continuation::ResumeChoice::NewFromHandoff)
        }
        crossterm::event::KeyCode::Char('f') => Some(crate::core::continuation::ResumeChoice::Full),
        _ => None,
    }
}

fn handle_mouse_left_click(mouse: crossterm::event::MouseEvent, view: &mut ViewState) -> Flow {
    let (x, y) = (mouse.column, mouse.row);
    if y == 0 {
        let mut current_x = 13u16; // " ✦ mjolnr │ "
        for surface in [
            crate::tui::workspace_types::WorkspaceSurface::Work,
            crate::tui::workspace_types::WorkspaceSurface::Conversation,
            crate::tui::workspace_types::WorkspaceSurface::Plan,
            crate::tui::workspace_types::WorkspaceSurface::Changes,
            crate::tui::workspace_types::WorkspaceSurface::Verify,
            crate::tui::workspace_types::WorkspaceSurface::Attention,
        ] {
            let label_len = u16::try_from(surface.label().len()).unwrap_or(0);
            let tab_len = if surface == view.active_surface {
                label_len + 5
            } else {
                label_len + 1
            };
            if x >= current_x && x < current_x + tab_len {
                view.active_surface = surface;
                return Flow::Redraw;
            }
            current_x += tab_len;
        }
    }
    Flow::Continue
}

/// Arrow keys and Enter drive the launcher's preset list while it is the
/// surface on screen.
///
/// Claims only those three keys, and only on an empty composer: everything
/// else — including the first character typed — falls through so the directive
/// band below keeps behaving exactly as it does on any other surface.
///
/// Enter applies the selected policy through the same `SetPolicy` command that
/// `Shift+Tab` dispatches. The launcher selects a mode; the runtime decides
/// what that mode permits.
async fn apply_launcher_key(
    key: crossterm::event::KeyEvent,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
) -> Option<Flow> {
    use crossterm::event::KeyCode;

    if !crate::tui::shell::should_render_launcher(view)
        || !view.composer.is_empty()
        || view.overlay != crate::tui::reducer::Overlay::None
        || view.snapshot.pending_approval.is_some()
        || view.snapshot.recovery.is_required()
        || view.snapshot.run_active
    {
        return None;
    }

    match key.code {
        KeyCode::Up => view.launcher.select_prev(),
        KeyCode::Down => view.launcher.select_next(),
        KeyCode::Enter => {
            let mode = view.launcher.selected_mode()?;
            if mode != view.snapshot.policy {
                let _ = runtime.dispatch(MjolnrCommand::SetPolicy { mode }).await;
            }
        }
        _ => return None,
    }
    Some(Flow::Redraw)
}

/// Keys belong to the jump palette while it is open.
///
/// `None` means the palette is not claiming this key and ordinary input
/// handling continues. The palette yields unconditionally to a gate that
/// arrived underneath it: an approval nobody can answer is a worse outcome
/// than a palette that closes itself.
fn apply_jump_palette_key(key: crossterm::event::KeyEvent, view: &mut ViewState) -> Option<Flow> {
    use crossterm::event::{KeyCode, KeyModifiers};

    if !view.jump_state.active {
        return None;
    }
    if view.snapshot.pending_approval.is_some() || view.snapshot.recovery.is_required() {
        view.jump_state.close();
        return None;
    }

    // Recomputed per keystroke rather than cached: the palette indexes live
    // view state, and a stale count would let the cursor point past the end of
    // what is actually on screen.
    let items = crate::tui::jump_palette::build_jump_items(view);
    let filtered = crate::tui::jump_palette::filter_jump_items(&items, &view.jump_state.query);

    match key.code {
        KeyCode::Esc => view.jump_state.close(),
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            view.jump_state.close();
        }
        KeyCode::Up => view.jump_state.move_cursor_up(filtered.len()),
        KeyCode::Down => view.jump_state.move_cursor_down(filtered.len()),
        KeyCode::Backspace => view.jump_state.backspace(),
        KeyCode::Enter => {
            let selected = filtered.get(view.jump_state.selected_index).cloned();
            view.jump_state.close();
            if let Some(item) = selected {
                apply_jump_selection(&item, view);
            }
        }
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            view.jump_state.input_char(character);
        }
        _ => return Some(Flow::Continue),
    }
    Some(Flow::Redraw)
}

/// Navigates to a chosen jump target.
///
/// Every arm changes only what the operator is looking at. Selecting a command
/// stages its text in the composer rather than running it, so the ordinary
/// submit path — and every gate on it — still applies.
fn apply_jump_selection(item: &crate::tui::jump_palette::JumpItem, view: &mut ViewState) {
    use crate::tui::jump_palette::JumpKind;
    use crate::tui::workspace_types::WorkspaceSurface;

    match item.kind {
        JumpKind::Surface => {
            if let Some(surface) = WorkspaceSurface::from_label(&item.target) {
                view.active_surface = surface;
            }
        }
        JumpKind::WorkItem => view.active_surface = WorkspaceSurface::Work,
        JumpKind::File => view.active_surface = WorkspaceSurface::Changes,
        JumpKind::Command => {
            view.clear_composer();
            view.append_composer(&item.target);
        }
        JumpKind::Fleet => {
            if let Some(child_id_str) = item.target.strip_prefix("fleet:") {
                view.focused_agent = view
                    .fleet
                    .iter()
                    .position(|a| a.child.to_string() == child_id_str);
            }
        }
    }
}

fn apply_text_editor_action(action: InputAction, view: &mut ViewState) -> Option<Flow> {
    match action {
        InputAction::Newline => {
            view.append_composer("\n");
            Some(Flow::Redraw)
        }
        InputAction::DeleteBackward => {
            view.delete_composer_character();
            Some(Flow::Redraw)
        }
        InputAction::Insert(character) => {
            let mut text = String::new();
            text.push(character);
            view.append_composer(&text);
            Some(Flow::Redraw)
        }
        InputAction::MoveCursorLeft => {
            view.move_cursor_left();
            Some(Flow::Redraw)
        }
        InputAction::MoveCursorRight => {
            view.move_cursor_right();
            Some(Flow::Redraw)
        }
        InputAction::MoveCursorWordLeft => {
            view.move_cursor_word_left();
            Some(Flow::Redraw)
        }
        InputAction::MoveCursorWordRight => {
            view.move_cursor_word_right();
            Some(Flow::Redraw)
        }
        InputAction::MoveToLineStart => {
            view.move_to_line_start();
            Some(Flow::Redraw)
        }
        InputAction::MoveToLineEnd => {
            view.move_to_line_end();
            Some(Flow::Redraw)
        }
        InputAction::DeleteForward => {
            view.delete_character_at_cursor();
            Some(Flow::Redraw)
        }
        InputAction::DeleteWordBackward => {
            view.delete_word_backward();
            Some(Flow::Redraw)
        }
        InputAction::DeleteWordForward => {
            view.delete_word_forward();
            Some(Flow::Redraw)
        }
        InputAction::DeleteToLineStart => {
            view.delete_to_line_start();
            Some(Flow::Redraw)
        }
        InputAction::DeleteToLineEnd => {
            view.delete_to_line_end();
            Some(Flow::Redraw)
        }
        InputAction::EditExternally => {
            if let Err(e) = edit_composer_externally(view) {
                view.model_notice = Some(format!("External editor failed: {e}"));
            }
            Some(Flow::HardRedraw)
        }
        InputAction::PasteClipboard => {
            paste_from_clipboard(view);
            Some(Flow::Redraw)
        }
        _ => None,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "TUI event action handler dispatches across all input actions"
)]
async fn apply_action(
    action: InputAction,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
    auth: &dyn AuthFlows,
) -> Flow {
    if let Some(flow) = apply_text_editor_action(action, view) {
        return flow;
    }
    match action {
        InputAction::Redraw | InputAction::ArmExit => Flow::Redraw,
        InputAction::Quit => Flow::Quit,
        InputAction::Interrupt => {
            // Cancel is a request, not a fact. The runtime decides, and the run
            // reports its own terminal event.
            view.cancelling = true;
            // Aborting returns the draft rather than eating it: text queued
            // against a turn the user just stopped was written for that turn,
            // and delivering it afterwards sends it into a context they never
            // saw.
            view.restore_queued_to_composer();
            let _ = runtime.dispatch(MjolnrCommand::CancelRun).await;
            Flow::Redraw
        }
        InputAction::ClearComposer => {
            view.clear_composer();
            Flow::Redraw
        }
        InputAction::Submit => submit(view, runtime).await,
        InputAction::ApprovePlan => {
            if let Some(command) = plan_approval_command(view) {
                let _ = runtime.dispatch(command).await;
            }
            Flow::Redraw
        }
        InputAction::QueueComposer => {
            view.queue_composer();
            Flow::Redraw
        }
        InputAction::SteerComposer => {
            let text = view.composer.trim().to_owned();
            if text.is_empty() {
                return Flow::Continue;
            }
            view.clear_composer();
            view.scroll_to_bottom();
            let _ = runtime
                .dispatch(MjolnrCommand::QueueSteeringMessage { text })
                .await;
            Flow::Redraw
        }
        InputAction::RecallQueuedComposer => {
            view.recall_queued_composer();
            Flow::Redraw
        }
        InputAction::CyclePolicy => {
            let _ = runtime
                .dispatch(MjolnrCommand::SetPolicy {
                    mode: view.snapshot.policy.next(),
                })
                .await;
            Flow::Redraw
        }
        InputAction::ToggleHelp => {
            view.toggle_help();
            Flow::Redraw
        }
        InputAction::ToggleToolDetails => {
            view.toggle_tool_details();
            Flow::Redraw
        }
        InputAction::CopyClipboard => {
            copy_to_clipboard(view).await;
            Flow::Redraw
        }
        InputAction::PickerUp
        | InputAction::PickerDown
        | InputAction::PickerCancel
        | InputAction::PickerConfirm
        | InputAction::CompleteCommand => apply_selection_action(action, view, runtime, auth).await,
        InputAction::ScrollUp => {
            view.scroll_up();
            Flow::Redraw
        }
        InputAction::ScrollDown => {
            view.scroll_down();
            Flow::Redraw
        }
        InputAction::ScrollBottom => {
            view.scroll_to_bottom();
            Flow::Redraw
        }
        InputAction::ResolveApproval(decision) => {
            let Some(approval) = view
                .snapshot
                .pending_approval
                .as_ref()
                .map(|pending| pending.id)
            else {
                return Flow::Continue;
            };
            let _ = runtime
                .dispatch(MjolnrCommand::ResolveApproval { approval, decision })
                .await;
            Flow::Redraw
        }
        InputAction::ResolveRecovery(decision) => {
            // Guarded on the snapshot rather than sent blind: the keymap
            // resolved this from a view that may be one frame stale, and the
            // runtime is the authority on whether anything needs resolving.
            if !view.snapshot.recovery.is_required() {
                return Flow::Continue;
            }
            let _ = runtime
                .dispatch(MjolnrCommand::ResolveRecovery { decision })
                .await;
            Flow::Redraw
        }
        InputAction::NextSurface => {
            view.next_surface();
            Flow::Redraw
        }
        InputAction::PreviousSurface => {
            view.previous_surface();
            Flow::Redraw
        }
        InputAction::SelectSurface(surface) => {
            view.active_surface = surface;
            Flow::Redraw
        }
        InputAction::ToggleJumpPalette => {
            // A gate owns the screen while it is up. The palette navigates, so
            // there is nowhere useful for it to go that does not first require
            // answering the question already on screen.
            if view.snapshot.pending_approval.is_some() || view.snapshot.recovery.is_required() {
                return Flow::Continue;
            }
            view.jump_state.toggle();
            Flow::Redraw
        }
        InputAction::JumpAttention => {
            view.jump_to_attention();
            Flow::Redraw
        }
        // `Continue` means the keymap deliberately swallowed the key. The rest
        // are consumed by `apply_text_editor_action`, which returns before this
        // match is reached. All are listed rather than swept up by a `_` arm:
        // the catch-all that used to sit here is exactly why
        // `ToggleJumpPalette` and the surface actions could be added, resolved
        // from a keypress, and silently do nothing. A new action must now fail
        // to compile until it is handled.
        InputAction::Continue
        | InputAction::Newline
        | InputAction::DeleteBackward
        | InputAction::Insert(_)
        | InputAction::MoveCursorLeft
        | InputAction::MoveCursorRight
        | InputAction::MoveCursorWordLeft
        | InputAction::MoveCursorWordRight
        | InputAction::MoveToLineStart
        | InputAction::MoveToLineEnd
        | InputAction::DeleteForward
        | InputAction::DeleteWordBackward
        | InputAction::DeleteWordForward
        | InputAction::DeleteToLineStart
        | InputAction::DeleteToLineEnd
        | InputAction::EditExternally
        | InputAction::PasteClipboard => Flow::Continue,
    }
}

fn plan_approval_command(view: &ViewState) -> Option<MjolnrCommand> {
    let workflow = view.snapshot.plan.as_ref()?;
    let (crate::core::plan::PlanStage::Proposed { proposal }
    | crate::core::plan::PlanStage::Reviewed { proposal, .. }) = &workflow.stage
    else {
        return None;
    };
    Some(MjolnrCommand::ApprovePlan {
        approval: crate::core::plan::PlanApproval {
            plan_id: workflow.plan_id,
            revision_id: proposal.revision_id,
            approver: "Human".to_owned(),
            decision: crate::core::plan::ReviewVerdict::Approve,
            note: None,
            approved_at: time::OffsetDateTime::now_utc(),
        },
    })
}

#[allow(clippy::cognitive_complexity)]
#[expect(
    clippy::too_many_lines,
    reason = "Slash command dispatcher matches built-in commands"
)]
async fn submit(view: &mut ViewState, runtime: &dyn MjolnrRuntime) -> Flow {
    let text = view.composer.trim().to_owned();
    if text.is_empty() {
        return Flow::Continue;
    }
    view.clear_composer();
    if text == "/help" || text == "/keymap" {
        view.toggle_help();
        return Flow::Redraw;
    }
    if text == "/skills" {
        view.toggle_skills();
        return Flow::Redraw;
    }
    if text == "/usage" {
        view.toggle_usage();
        return Flow::Redraw;
    }
    if text == "/auth" || text == "/login" || text == "/provider" {
        view.toggle_auth();
        return Flow::Redraw;
    }
    if text == "/config" {
        view.toggle_config();
        return Flow::Redraw;
    }
    if text == "/theme"
        || text.starts_with("/theme ")
        || text == "/palette"
        || text.starts_with("/palette ")
    {
        let requested = text
            .strip_prefix("/theme")
            .or_else(|| text.strip_prefix("/palette"))
            .unwrap_or_default()
            .trim();
        if requested.is_empty() {
            view.toggle_theme();
        } else if let Some(theme_id) = crate::tui::theme::ThemeId::parse(requested) {
            view.close_overlay();
            crate::tui::theme::set_active_theme_id(theme_id);
            persist_theme(theme_id);
        } else {
            view.note_model_command_failure(
                "UNKNOWN_THEME — available: zeppi, zeppi-light, noir, mono",
            );
        }
        return Flow::Redraw;
    }
    if text == "/mcp" {
        view.toggle_mcp();
        return Flow::Redraw;
    }
    if text == "/triggers" {
        view.toggle_triggers();
        return Flow::Redraw;
    }
    if text == "/memory" {
        view.toggle_memory();
        return Flow::Redraw;
    }
    if text == "/plugins" {
        view.toggle_plugins();
        return Flow::Redraw;
    }
    if text == "/external" || text == "/agents" {
        view.toggle_external_agents();
        return Flow::Redraw;
    }
    if text == "/tree" {
        view.toggle_tree();
        // Opening reads the tree, including the branches this session is not
        // on — the runtime replays only the active branch, so nothing else
        // would ever put a sibling on the snapshot.
        if view.overlay == crate::tui::reducer::Overlay::Tree {
            let _ = runtime.dispatch(MjolnrCommand::LoadSessionTree).await;
        }
        return Flow::Redraw;
    }
    if text == "/clone" {
        return clone_command(view, runtime).await;
    }
    if text == "/fork" || text.starts_with("/fork ") {
        return fork_command(
            text.strip_prefix("/fork").unwrap_or_default(),
            view,
            runtime,
        )
        .await;
    }
    if text == "/handoff" || text.starts_with("/handoff ") {
        view.close_overlay();
        let target = text.strip_prefix("/handoff").unwrap_or_default().trim();
        let target_opt = if target.is_empty() {
            None
        } else {
            Some(target.to_owned())
        };
        let _ = runtime
            .dispatch(MjolnrCommand::CreateHandoff { target: target_opt })
            .await;
        return Flow::Redraw;
    }
    if text == "/council" || text.starts_with("/council ") {
        view.close_overlay();
        let body = text.strip_prefix("/council").unwrap_or_default().trim();
        let (question, plan_file) = if let Some(path) = body.strip_prefix("plan ") {
            ("Review plan".to_string(), Some(path.trim().to_string()))
        } else {
            (body.to_string(), None)
        };
        let _ = runtime
            .dispatch(MjolnrCommand::ConveneCouncil {
                question,
                plan_file,
            })
            .await;
        return Flow::Redraw;
    }
    if text == "/plan" {
        view.close_overlay();
        view.active_surface = crate::tui::workspace_types::WorkspaceSurface::Plan;
        return Flow::Redraw;
    }
    if text.starts_with("/plan ") {
        let goal = text.strip_prefix("/plan ").unwrap_or_default().trim();
        if goal.is_empty() {
            view.note_model_command_failure("PLAN_GOAL_REQUIRED — use /plan <goal>");
            return Flow::Redraw;
        }
        view.close_overlay();
        view.active_surface = crate::tui::workspace_types::WorkspaceSurface::Plan;
        let _ = runtime
            .dispatch(MjolnrCommand::StartPlanInterview {
                goal: goal.to_owned(),
            })
            .await;
        return Flow::Redraw;
    }
    if text == "/policy" || text.starts_with("/policy ") {
        return set_policy_command(
            text.strip_prefix("/policy").unwrap_or_default(),
            view,
            runtime,
        )
        .await;
    }
    if text == "/envelope" || text.starts_with("/envelope ") {
        return envelope_command(
            text.strip_prefix("/envelope").unwrap_or_default(),
            view,
            runtime,
        )
        .await;
    }
    if text == "/route" || text.starts_with("/route ") {
        return route_command(
            crate::tui::commands::RouteBy::Name,
            text.strip_prefix("/route").unwrap_or_default(),
            view,
            runtime,
        )
        .await;
    }
    if text == "/role" || text.starts_with("/role ") {
        return route_command(
            crate::tui::commands::RouteBy::Role,
            text.strip_prefix("/role").unwrap_or_default(),
            view,
            runtime,
        )
        .await;
    }
    if text == "/persona" || text.starts_with("/persona ") {
        return persona_command(
            text.strip_prefix("/persona").unwrap_or_default(),
            view,
            runtime,
        )
        .await;
    }
    if text == "/soul" {
        return soul_command(view);
    }
    if text == "/model" || text == "/models" || text.starts_with("/model ") {
        let requested = text.strip_prefix("/model").unwrap_or_default();
        let mut parts = requested.split_whitespace();
        let pair = (parts.next(), parts.next(), parts.next());
        match pair {
            // Explicit pair: set it directly, for people who know the strings
            // and for anything scripted.
            (Some(provider), Some(model), None) => {
                view.close_overlay();
                view.scroll_to_bottom();
                let _ = runtime
                    .dispatch(MjolnrCommand::SelectModel {
                        provider: ProviderId::new(provider),
                        model: ModelId::new(model),
                    })
                    .await;
            }
            // Bare command: show the choices. Refusing here would demand the
            // user already know the exact provider/model strings, which is
            // precisely what they opened the command to find out.
            (None, _, _) => view.toggle_models(),
            _ => {
                view.note_model_command_failure(
                    "SCHEMA_INVALID — use /model, or /model <provider> <model>",
                );
            }
        }
        return Flow::Redraw;
    }
    if text == "/reload" {
        view.close_overlay();
        let _ = runtime.dispatch(MjolnrCommand::ReloadResources).await;
        // The runtime reloads synchronously and republishes; the notice is
        // rendered from the snapshot on the next frame.
        return Flow::Redraw;
    }
    if text == "/discover" {
        view.toggle_discovery();
        if view.overlay == crate::tui::reducer::Overlay::Discovery {
            match runtime.dispatch(MjolnrCommand::RunDiscovery).await {
                Ok(()) => view.note_model_command_notice(
                    "DISCOVERY COMPLETE — durable OKF bundle written; proposal remains owner-editable",
                ),
                Err(error) => view.note_model_command_failure(&error.to_string()),
            }
        }
        return Flow::Redraw;
    }
    if text == "/load-extension" || text.starts_with("/load-extension ") {
        view.close_overlay();
        let name = text
            .strip_prefix("/load-extension")
            .unwrap_or_default()
            .trim();
        if name.is_empty() {
            view.note_model_command_failure(
                "SCHEMA_INVALID — use /load-extension <extension name>",
            );
            return Flow::Redraw;
        }
        let _ = runtime
            .dispatch(MjolnrCommand::LoadExtension {
                name: name.to_owned(),
            })
            .await;
        // The runtime loads synchronously and republishes; the outcome renders
        // from the snapshot on the next frame.
        return Flow::Redraw;
    }
    // Session lifecycle commands. "Leave" releases the seat without ending the
    // session (it stays resumable). "End" is terminal: an ended session cannot
    // accept new work. The runtime refuses both while a run is active or
    // recovery is pending, and the refusal lands on the durability banner.
    if text == "/leave" {
        view.close_overlay();
        let _ = runtime.dispatch(MjolnrCommand::ReleaseSession).await;
        return Flow::Redraw;
    }
    if text == "/end" {
        view.close_overlay();
        let _ = runtime.dispatch(MjolnrCommand::EndSession).await;
        return Flow::Redraw;
    }
    if text == "/reclaim" || text.starts_with("/reclaim ") {
        view.close_overlay();
        let argument = text.strip_prefix("/reclaim").unwrap_or_default().trim();
        if argument.is_empty() {
            // Find the first stale lease from the snapshot.
            let stale = view
                .snapshot
                .sessions
                .iter()
                .find(|s| s.leased)
                .map(|s| s.id.clone());
            match stale {
                Some(session) => {
                    let _ = runtime
                        .dispatch(MjolnrCommand::ReclaimSession { session })
                        .await;
                }
                None => {
                    view.note_model_command_failure(
                        "NO_STALE_LEASE — no session has a stale lease to reclaim",
                    );
                }
            }
        } else {
            // Match by prefix against known sessions.
            let prefix = argument.to_lowercase();
            let candidate = view
                .snapshot
                .sessions
                .iter()
                .find(|s| {
                    let id = s.id.to_string();
                    id.to_lowercase().starts_with(&prefix)
                })
                .map(|s| s.id.clone());
            match candidate {
                Some(session) => {
                    let _ = runtime
                        .dispatch(MjolnrCommand::ReclaimSession { session })
                        .await;
                }
                None => {
                    view.note_model_command_failure(&format!(
                        "NO_MATCH — no session id starts with `{argument}`"
                    ));
                }
            }
        }
        return Flow::Redraw;
    }
    // A prompt template expands into the user message and is sent as if the
    // user had typed it. Built-ins were all matched above, so a template can
    // never shadow one — it only ever reaches here for a name none of them
    // claimed. Expansion happens in the runtime, which owns the template text;
    // the view knows only that the name exists.
    if let Some((name, arguments)) = prompt_template_invocation(&text, view) {
        view.close_overlay();
        view.scroll_to_bottom();
        let _ = runtime
            .dispatch(MjolnrCommand::SendPromptTemplate { name, arguments })
            .await;
        return Flow::Redraw;
    }
    let pending_question = (!text.starts_with('/')).then(|| {
        view.snapshot.plan.as_ref().and_then(|workflow| {
            let crate::core::plan::PlanStage::QuestionPending { question } = &workflow.stage else {
                return None;
            };
            Some((workflow.plan_id, question.id))
        })
    });
    if let Some(Some((plan_id, question_id))) = pending_question {
        view.close_overlay();
        let _ = runtime
            .dispatch(MjolnrCommand::AnswerPlanQuestion {
                plan_id,
                answer: crate::core::plan::QuestionAnswer {
                    question_id,
                    selected_options: Vec::new(),
                    freeform_text: Some(text),
                    answered_at: time::OffsetDateTime::now_utc(),
                },
            })
            .await;
        return Flow::Redraw;
    }
    // An unknown slash command is refused rather than sent to the model as
    // prose: a typo'd command reaching the provider spends tokens to be told
    // it made no sense.
    if text.starts_with('/') {
        let name = text.split_whitespace().next().unwrap_or(&text).to_owned();
        view.note_model_command_failure(&format!(
            "UNKNOWN_COMMAND — no built-in or prompt template named {name}"
        ));
        return Flow::Redraw;
    }
    view.close_overlay();
    view.scroll_to_bottom();
    let _ = runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text,
            // The composer is the human, by definition.
            source: crate::core::directive::DirectiveSource::Human,
        })
        .await;
    Flow::Redraw
}

/// Split `/name [arguments]` when `name` is a discovered template.
///
/// Returns `None` when the text is not a slash command or names no template,
/// leaving the caller to decide what that means.
fn prompt_template_invocation(text: &str, view: &ViewState) -> Option<(String, String)> {
    let rest = text.strip_prefix('/')?;
    let (name, arguments) = match rest.split_once(char::is_whitespace) {
        Some((name, arguments)) => (name, arguments.trim()),
        None => (rest, ""),
    };
    view.snapshot
        .prompts
        .iter()
        .find(|template| template.name == name)
        .map(|template| (template.name.clone(), arguments.to_owned()))
}

/// Drive the `/config` settings surface : navigate rows, cycle
/// a value into a staged preview, then write it to the diffable file that owns
/// it. Nothing here gates work or records a policy event — it edits config.
async fn apply_config_action(
    action: InputAction,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
) -> Flow {
    use crate::tui::reducer::ConfigStaged;
    match action {
        InputAction::PickerUp => view.move_config_cursor(-1),
        InputAction::PickerDown => view.move_config_cursor(1),
        // Space cycles the focused setting's value, staging a preview.
        InputAction::Insert(' ') => view.cycle_config_value(),
        // Enter writes the staged change to the diffable file that owns it.
        InputAction::PickerConfirm => match view.take_config_staged() {
            Some(ConfigStaged::RoutePersona { route, persona }) => {
                let _ = runtime
                    .dispatch(MjolnrCommand::BindRoutePersona { route, persona })
                    .await;
            }
            Some(ConfigStaged::Theme { theme }) => {
                crate::tui::theme::set_active_theme_id(theme);
                persist_theme(theme);
            }
            None => {}
        },
        // Esc discards a staged change first, then closes — a preview is not a
        // commitment, and backing out of one must never touch a file.
        InputAction::PickerCancel => {
            if view.config_staged.is_some() {
                view.clear_config_staged();
            } else {
                view.close_overlay();
            }
        }
        _ => {}
    }
    Flow::Redraw
}

async fn apply_selection_action(
    action: InputAction,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
    auth: &dyn AuthFlows,
) -> Flow {
    if view.overlay == crate::tui::reducer::Overlay::Config {
        return apply_config_action(action, view, runtime).await;
    }

    if view.overlay == crate::tui::reducer::Overlay::Discovery {
        if matches!(action, InputAction::PickerCancel) {
            view.close_overlay();
        }
        return Flow::Redraw;
    }

    if view.overlay == crate::tui::reducer::Overlay::Theme {
        match action {
            InputAction::PickerUp => view.move_theme_cursor(-1),
            InputAction::PickerDown => view.move_theme_cursor(1),
            InputAction::PickerCancel => {
                view.close_overlay();
            }
            InputAction::PickerConfirm => {
                if let Some(theme_id) = crate::tui::theme::ThemeId::all().get(view.theme_cursor) {
                    crate::tui::theme::set_active_theme_id(*theme_id);
                    persist_theme(*theme_id);
                    view.close_overlay();
                }
            }
            _ => {}
        }
        return Flow::Redraw;
    }

    if view.overlay == crate::tui::reducer::Overlay::Auth {
        match action {
            InputAction::PickerUp => view.move_auth_cursor(-1),
            InputAction::PickerDown => view.move_auth_cursor(1),
            InputAction::PickerCancel => {
                view.close_overlay();
            }
            InputAction::PickerConfirm => {
                if let Some(provider) = view
                    .selected_auth_provider()
                    .map(|c| c.provider.as_str().to_owned())
                {
                    return handle_auth_login_command(&provider, view, runtime, auth).await;
                }
            }
            _ => {}
        }
        return Flow::Redraw;
    }

    if view.overlay == crate::tui::reducer::Overlay::Tree {
        match action {
            InputAction::PickerUp => view.tree_cursor_up(),
            InputAction::PickerDown => view.tree_cursor_down(),
            InputAction::PickerCancel => {
                view.close_overlay();
            }
            InputAction::PickerConfirm => return confirm_tree_rewind(view, runtime).await,
            _ => {}
        }
        return Flow::Redraw;
    }

    match action {
        InputAction::PickerUp => view.move_model_cursor(-1),
        InputAction::PickerDown => view.move_model_cursor(1),
        InputAction::PickerCancel => {
            view.close_overlay();
            view.clear_composer();
        }
        InputAction::PickerConfirm => return confirm_model_pick(view, runtime).await,
        InputAction::CompleteCommand => {
            // Completes only when exactly one command matches. Two candidates
            // mean the user has not chosen yet, and picking for them would send
            // a different command than they were typing.
            let matches = crate::tui::commands::menu_entries(&view.composer, view);
            if let [only] = matches.as_slice() {
                let name = only.name.clone();
                view.set_composer(&name);
            }
        }
        _ => {}
    }
    Flow::Redraw
}

fn initialize_theme() {
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let colorterm = std::env::var("COLORTERM").ok();
    let term = std::env::var("TERM").ok();
    let detected =
        crate::tui::theme::detect_color_depth(no_color, colorterm.as_deref(), term.as_deref());
    crate::tui::theme::set_detected_color_depth(detected.depth);
    if detected.force_mono {
        crate::tui::theme::set_active_theme_id(crate::tui::theme::ThemeId::Mono);
    } else if let Some(theme) = theme_path().and_then(|path| read_theme(&path)) {
        crate::tui::theme::set_active_theme_id(theme);
    }
}

fn theme_path() -> Option<PathBuf> {
    crate::core::paths::resolve_user_config_dir().map(|directory| directory.join("theme"))
}

fn read_theme(path: &Path) -> Option<crate::tui::theme::ThemeId> {
    let value = std::fs::read_to_string(path).ok()?;
    crate::tui::theme::ThemeId::parse(value.trim())
}

fn persist_theme(theme: crate::tui::theme::ThemeId) {
    let Some(path) = theme_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let _ = std::fs::write(path, theme.name());
}

/// Commit the highlighted connected model.
async fn confirm_model_pick(view: &mut ViewState, runtime: &dyn MjolnrRuntime) -> Flow {
    let Some(choice) = view.selected_model() else {
        view.note_model_command_failure("NO_MATCH — no model matches that filter");
        return Flow::Redraw;
    };

    let provider = choice.descriptor.provider.clone();
    let model = choice.descriptor.id.clone();
    view.close_overlay();
    view.clear_composer();
    view.scroll_to_bottom();
    let _ = runtime
        .dispatch(MjolnrCommand::SelectModel { provider, model })
        .await;
    Flow::Redraw
}

/// Rewind to the selected turn and hand its text back for editing (plan
/// §Phase 16.5).
///
/// The order matters. The rewind goes first, so the composer is seeded from a
/// turn that has already left the branch — seeding first and rewinding after
/// happens to produce the same string today, but only because nothing between
/// the two can change it, which is not a property worth depending on.
///
/// Both refusals below are visible. `rewind_to` returns nothing and no-ops when
/// it cannot act, so a silent dispatch would leave the user looking at an
/// overlay that closed and a session that did not move.
async fn confirm_tree_rewind(view: &mut ViewState, runtime: &dyn MjolnrRuntime) -> Flow {
    let Some(row) = view.selected_tree_row() else {
        view.note_model_command_failure("NO_TURN — there is nothing to branch from yet");
        return Flow::Redraw;
    };

    // A run in flight owns the transcript it is appending to. The runtime
    // refuses a rewind during one; saying so beats dispatching into a no-op.
    if view.snapshot.run_active {
        view.note_model_command_failure("RUN_ACTIVE — stop the turn before branching from it");
        return Flow::Redraw;
    }

    let Some(sequence) = row.sequence else {
        view.note_model_command_failure(
            "NO_BRANCH_POINT — this message was restored from a checkpoint, \
             so history has no point to branch from",
        );
        return Flow::Redraw;
    };

    view.close_overlay();

    // Enter means two different things, decided by where the row sits.
    //
    // On the branch being followed, it means "branch away from here": rewind to
    // before this turn and hand its text back for editing. On a branch that was
    // left behind, it means "go back to that": follow it again, and leave the
    // composer alone, because the user is returning to work rather than
    // rewriting a prompt.
    //
    // Collapsing the two into one action would make selecting an abandoned turn
    // rewind the branch you are on to a point that is not even on it.
    if row.on_active_branch {
        let _ = runtime.dispatch(MjolnrCommand::RewindTo { sequence }).await;
        view.set_composer(&row.prompt);
    } else {
        let _ = runtime
            .dispatch(MjolnrCommand::FollowBranch { sequence })
            .await;
    }
    view.scroll_to_bottom();
    Flow::Redraw
}

/// `/clone` — duplicate the active branch into a session of its own.
async fn clone_command(view: &mut ViewState, runtime: &dyn MjolnrRuntime) -> Flow {
    view.close_overlay();
    if view.snapshot.run_active {
        view.note_model_command_failure("RUN_ACTIVE — finish the turn before cloning it");
        return Flow::Redraw;
    }
    let _ = runtime
        .dispatch(MjolnrCommand::ForkSession { before: None })
        .await;
    Flow::Redraw
}

/// `/fork N` — start a new session from turn `N` as `/tree` numbers it.
///
/// Numbered rather than addressed by sequence because the numbers are what the
/// overlay actually shows. A raw event sequence is the honest identifier and a
/// useless one to type.
async fn fork_command(argument: &str, view: &mut ViewState, runtime: &dyn MjolnrRuntime) -> Flow {
    let rows = view.tree_rows();
    let argument = argument.trim();

    if argument.is_empty() {
        view.note_model_command_failure(
            "NEEDS_TURN — `/fork N` forks at turn N; open /tree to see the numbers",
        );
        return Flow::Redraw;
    }
    let Ok(number) = argument.parse::<usize>() else {
        view.note_model_command_failure(&format!(
            "NOT_A_TURN — `{argument}` is not a turn number; open /tree to see them"
        ));
        return Flow::Redraw;
    };
    // The overlay numbers rows from one.
    let Some(row) = number.checked_sub(1).and_then(|index| rows.get(index)) else {
        view.note_model_command_failure(&format!(
            "NO_SUCH_TURN — there is no turn {number}; /tree lists {} of them",
            rows.len()
        ));
        return Flow::Redraw;
    };
    let Some(sequence) = row.sequence else {
        view.note_model_command_failure(
            "NO_BRANCH_POINT — that turn was restored from a checkpoint, \
             so history has no point to fork from",
        );
        return Flow::Redraw;
    };

    if view.snapshot.run_active {
        view.note_model_command_failure("RUN_ACTIVE — finish the turn before forking from it");
        return Flow::Redraw;
    }

    let prompt = row.prompt.clone();
    view.close_overlay();
    let _ = runtime
        .dispatch(MjolnrCommand::ForkSession {
            before: Some(sequence),
        })
        .await;
    // The forked turn is what the user wants to say differently, so it comes
    // back for editing — the same hand-back a rewind does, in a new session.
    view.set_composer(&prompt);
    Flow::Redraw
}

/// Attach a route or role to the idle session, or explain why it cannot.
///
/// The decision is made purely in [`plan_route_command`] against the offered
/// choices, so an unknown name is a stated notice rather than a silent
/// `AttachRoute` no-op. Attachment repoints the session's provider/model, which
/// a live turn holds a reference to, so it is refused while a run is active —
/// the same idle-only rule `/model` and `/policy` follow.
async fn route_command(
    by: crate::tui::commands::RouteBy,
    argument: &str,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
) -> Flow {
    view.close_overlay();
    if view.snapshot.run_active {
        view.note_model_command_failure("cannot attach a route while a run is active");
        return Flow::Redraw;
    }
    match crate::tui::commands::plan_route_command(by, argument, &view.snapshot) {
        crate::tui::commands::RoutePlan::Attach { route, role } => {
            view.scroll_to_bottom();
            let _ = runtime
                .dispatch(MjolnrCommand::AttachRoute {
                    route,
                    role,
                    task_class: "default".to_owned(),
                })
                .await;
        }
        crate::tui::commands::RoutePlan::Notice(text) => {
            view.note_model_command_failure(&text);
        }
    }
    Flow::Redraw
}

/// Overlay a persona on the idle session, clear the override, or explain why
/// not. The decision is made in [`plan_persona_command`]
/// against the offered personas, so an unknown name is a stated notice rather
/// than a silent no-op. It changes the next turn's system prompt, so — like
/// `/route` and `/model` — it is refused while a run is active.
async fn persona_command(
    argument: &str,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
) -> Flow {
    view.close_overlay();
    if view.snapshot.run_active {
        view.note_model_command_failure("cannot change persona while a run is active");
        return Flow::Redraw;
    }
    match crate::tui::commands::plan_persona_command(argument, &view.snapshot) {
        crate::tui::commands::PersonaPlan::Select(persona) => {
            view.scroll_to_bottom();
            let _ = runtime
                .dispatch(MjolnrCommand::SelectPersona { persona })
                .await;
        }
        crate::tui::commands::PersonaPlan::Notice(text) => {
            view.note_model_command_failure(&text);
        }
    }
    Flow::Redraw
}

/// Show the Soul/profile files in effect and the active persona (
/// 23). A view of the record: it reads state and changes nothing.
fn soul_command(view: &mut ViewState) -> Flow {
    view.close_overlay();
    let persona = view.snapshot.active_persona.as_deref().map_or_else(
        || "persona: none".to_owned(),
        |name| format!("persona: {name}"),
    );
    let identity = if view.snapshot.souls.is_empty() {
        "no SOUL.md or USER.md loaded".to_owned()
    } else {
        view.snapshot.souls.join("; ")
    };
    view.note_model_command_failure(&format!("{identity} · {persona}"));
    Flow::Redraw
}

async fn set_policy_command(
    requested: &str,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
) -> Flow {
    use crate::core::policy::PolicyMode;

    if view.snapshot.run_active {
        view.note_model_command_failure("POLICY LOCKED — policy changes only while idle");
        return Flow::Redraw;
    }
    let mode = match requested.trim() {
        "read-only" => Some(PolicyMode::ReadOnly),
        "ask" => Some(PolicyMode::Ask),
        "workspace-write" => Some(PolicyMode::WorkspaceWrite),
        "full-auto" => {
            view.full_auto_armed = true;
            return Flow::Redraw;
        }
        _ => None,
    };
    match mode {
        Some(mode) => {
            let _ = runtime.dispatch(MjolnrCommand::SetPolicy { mode }).await;
        }
        None => view.note_model_command_failure(
            "SCHEMA_INVALID — use /policy read-only|ask|workspace-write|full-auto",
        ),
    }
    Flow::Redraw
}

/// `/envelope` — show, arm, or clear the spawn envelope.
///
/// Arming goes through the same one-key confirmation full-auto uses, and for
/// the same reason: it is a grant of authority over acts that have not been
/// proposed yet, so it should cost a deliberate keystroke rather than an Enter
/// pressed by habit.
async fn envelope_command(
    requested: &str,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
) -> Flow {
    use crate::core::envelope::SpawnEnvelope;

    let requested = requested.trim();
    if requested.is_empty() {
        // Bare `/envelope` reports rather than acts. Asking what is currently
        // authorised must never be a way to authorise something.
        match view.snapshot.envelope.as_ref() {
            Some(active) => view.note_model_command_notice(&format!(
                "ENVELOPE — {} of {} children left, {} turns of budget, ceiling {}",
                active.children_remaining(),
                active.envelope.max_children,
                active.turns_remaining(),
                active.envelope.ceiling.label()
            )),
            None => view.note_model_command_notice("ENVELOPE — none armed"),
        }
        return Flow::Redraw;
    }
    if requested == "off" {
        let _ = runtime.dispatch(MjolnrCommand::ClearSpawnEnvelope).await;
        return Flow::Redraw;
    }
    if view.snapshot.run_active {
        view.note_model_command_failure("ENVELOPE LOCKED — arm one only while idle");
        return Flow::Redraw;
    }

    let mut fields = requested.split_whitespace();
    let Some(children) = fields.next().and_then(|raw| raw.parse::<u32>().ok()) else {
        view.note_model_command_failure(
            "SCHEMA_INVALID — use /envelope <children> [ceiling] [turns], or /envelope off",
        );
        return Flow::Redraw;
    };
    let ceiling = match fields.next() {
        None | Some("read-only") => crate::core::policy::PolicyMode::ReadOnly,
        Some("workspace-write") => crate::core::policy::PolicyMode::WorkspaceWrite,
        Some("full-auto") => crate::core::policy::PolicyMode::FullAuto,
        Some(_) => {
            view.note_model_command_failure(
                "SCHEMA_INVALID — ceiling is read-only|workspace-write|full-auto",
            );
            return Flow::Redraw;
        }
    };
    let turns = fields
        .next()
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(20);

    let envelope = SpawnEnvelope::for_children(children, ceiling, turns);
    if let Err(refusal) = envelope.validate(view.snapshot.policy) {
        view.note_model_command_failure(&format!("ENVELOPE REFUSED — {}", refusal.detail()));
        return Flow::Redraw;
    }
    view.envelope_armed = Some(Box::new(envelope));
    Flow::Redraw
}

fn paste_from_clipboard(view: &mut ViewState) {
    let workspace_root = view.snapshot.workspace_root.as_ref();
    if let Some(image_path) = workspace_root.and_then(|root| try_paste_clipboard_image(root)) {
        let md_link = format!("![pasted_image](file://{})", image_path.display());
        view.append_composer(&md_link);
        return;
    }

    if let Some(text) = try_paste_clipboard_text() {
        view.append_composer(&text);
    }
}

async fn copy_to_clipboard(view: &mut ViewState) {
    let Some(text) = clipboard_payload(view) else {
        view.model_notice = Some("Nothing available to copy".to_owned());
        return;
    };
    let result = tokio::task::spawn_blocking(move || write_clipboard_text(&text)).await;
    match result {
        Ok(Ok(())) => view.model_notice = Some("Copied to clipboard".to_owned()),
        Ok(Err(error)) => {
            view.model_notice = Some(format!("Clipboard copy failed: {error}"));
        }
        Err(error) => {
            view.model_notice = Some(format!("Clipboard copy failed: {error}"));
        }
    }
}

fn clipboard_payload(view: &ViewState) -> Option<String> {
    if !view.composer.is_empty() {
        return Some(view.composer.clone());
    }
    if let crate::tui::reducer::RunStatus::Failed { code, detail } = &view.status {
        return Some(format!("{code}: {detail}"));
    }
    view.snapshot
        .messages
        .iter()
        .rev()
        .find(|message| message.role == crate::core::message::Role::Assistant)
        .map(|message| message.text())
        .filter(|text| !text.is_empty())
}

#[cfg(target_os = "macos")]
fn write_clipboard_text(text: &str) -> io::Result<()> {
    let mut child = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    let Some(mut input) = child.stdin.take() else {
        return Err(io::Error::other("pbcopy did not open stdin"));
    };
    std::io::Write::write_all(&mut input, text.as_bytes())?;
    drop(input);
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("pbcopy exited unsuccessfully"))
    }
}

#[cfg(not(target_os = "macos"))]
fn write_clipboard_text(_text: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "clipboard copy is currently available on macOS",
    ))
}

// `workspace_root` is read only by the macOS body; every other target
// short-circuits to `None`, so the binding is genuinely unused there.
#[cfg_attr(
    not(target_os = "macos"),
    expect(unused_variables, reason = "clipboard image paste is macOS-only")
)]
fn try_paste_clipboard_image(workspace_root: &std::path::Path) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let assets_dir = workspace_root.join(".mjolnr").join("assets");
        if std::fs::create_dir_all(&assets_dir).is_ok() {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let filename = format!("paste_{timestamp}.png");
            let dest_path = assets_dir.join(&filename);

            let script = format!(
                "try\n\
                  set pngData to the clipboard as «class PNGf»\n\
                  set f to open for access POSIX file \"{}\" with write permission\n\
                  set eof f to 0\n\
                  write pngData to f\n\
                  close access f\n\
                  true\n\
                on error\n\
                  try\n\
                    close access POSIX file \"{}\"\n\
                  end try\n\
                  false\n\
                end try",
                dest_path.to_string_lossy(),
                dest_path.to_string_lossy()
            );

            let output = std::process::Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .output();

            if let Ok(output) = output {
                let result_str = String::from_utf8_lossy(&output.stdout);
                if result_str.trim() == "true" && dest_path.exists() {
                    return Some(dest_path);
                }
            }
        }
    }
    None
}

fn try_paste_clipboard_text() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("pbpaste").output().ok()?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn edit_composer_externally(view: &mut ViewState) -> io::Result<()> {
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    let _ = ratatui::try_restore();

    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "nano".to_string());

    let temp_dir = std::env::temp_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp_file_path = temp_dir.join(format!("mjolnr_composer_{timestamp}.txt"));

    std::fs::write(&temp_file_path, view.composer.as_bytes())?;

    let status = std::process::Command::new(&editor)
        .arg(&temp_file_path)
        .status()?;

    if status.success() {
        let content = std::fs::read_to_string(&temp_file_path)?;
        content.trim_end().clone_into(&mut view.composer);
        view.composer_cursor = view.composer.chars().count();
    }

    let _ = std::fs::remove_file(&temp_file_path);

    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::cursor::Hide,
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableMouseCapture
    )?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    )?;

    Ok(())
}

/// What a completed suspended-terminal auth flow left behind.
enum AuthFlowOutcome {
    /// An API key was captured; the runtime stores it.
    StoredApiKey(String),
    /// An OAuth flow wrote the credential store directly; only a refresh is needed.
    StoredOAuth,
    /// A keyless local provider only needs its endpoint rediscovered.
    RefreshOnly,
    Aborted,
}

#[allow(clippy::print_stdout)]
async fn handle_auth_login_command(
    provider_str: &str,
    view: &mut ViewState,
    runtime: &dyn MjolnrRuntime,
    auth: &dyn AuthFlows,
) -> Flow {
    let provider_lower = provider_str.trim().to_lowercase();
    if provider_lower.is_empty() {
        view.note_model_command_failure("SCHEMA_INVALID — use /auth login <provider>");
        return Flow::Redraw;
    }

    suspend_terminal();
    println!("\n=== MJOLNR CREDENTIAL REGISTER ===");
    let outcome = run_auth_flow(&provider_lower, auth).await;
    println!("\nPress Enter to return to mjolnr...");
    let mut dummy = String::new();
    let _ = std::io::stdin().read_line(&mut dummy);
    restore_terminal();

    match outcome {
        AuthFlowOutcome::StoredApiKey(secret) => {
            // Dispatch credential storage command to runtime actor
            let _ = runtime
                .dispatch(crate::core::command::MjolnrCommand::RegisterCredential {
                    provider: crate::core::model::ProviderId::new(&provider_lower),
                    secret: crate::core::command::CredentialSecret(secret),
                })
                .await;
        }
        AuthFlowOutcome::StoredOAuth | AuthFlowOutcome::RefreshOnly => {
            let _ = runtime
                .dispatch(crate::core::command::MjolnrCommand::RefreshCredentials)
                .await;
        }
        AuthFlowOutcome::Aborted => return Flow::HardRedraw,
    }

    view.composer.clear();
    view.composer_cursor = 0;
    view.close_overlay();
    Flow::HardRedraw
}

/// Route a provider to its real login flow while the terminal is suspended.
///
/// The TUI knows only which *shape* of login a provider id uses; the flows
/// themselves are injected ( — the TUI may never call a provider).
#[allow(clippy::print_stdout)]
async fn run_auth_flow(provider: &str, auth: &dyn AuthFlows) -> AuthFlowOutcome {
    match provider {
        "anthropic" => {
            println!("anthropic holds one of two credentials:");
            println!("  [1] Claude Pro/Max subscription login (uses your plan)");
            println!("  [2] API key (metered billing)");
            print!("Choose [1/2] (default 1): ");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let mut choice = String::new();
            let _ = std::io::stdin().read_line(&mut choice);
            if choice.trim().starts_with('2') {
                api_key_flow(provider)
            } else {
                report_oauth_outcome(auth.oauth_login(provider).await)
            }
        }
        "openai-codex" | "gemini-cli" | "antigravity" => {
            report_oauth_outcome(auth.oauth_login(provider).await)
        }
        "lm-studio" => optional_api_key_flow(auth),
        "ollama" => {
            println!("mjolnr will check the local Ollama server and installed models.");
            println!("If it is stopped, run: ollama serve");
            AuthFlowOutcome::RefreshOnly
        }
        _ => api_key_flow(provider),
    }
}

#[allow(clippy::print_stdout)]
fn optional_api_key_flow(auth: &dyn AuthFlows) -> AuthFlowOutcome {
    print!("LM Studio server IP or URL [blank keeps current]: ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut address = String::new();
    if let Err(error) = std::io::stdin().read_line(&mut address) {
        println!("Error: could not read server address: {error}");
        return AuthFlowOutcome::Aborted;
    }
    match auth.configure_lm_studio_endpoint(address.trim()) {
        Ok(endpoint) => println!("Saved LM Studio endpoint: {endpoint}"),
        Err(error) => {
            println!("Error: {error}");
            return AuthFlowOutcome::Aborted;
        }
    }

    println!("LM Studio is keyless by default.");
    println!("If its server requires authentication, enter an LM Studio API token.");
    let entered = match rpassword::prompt_password("API token (blank for keyless): ") {
        Ok(value) => value,
        Err(error) => {
            println!("Error: could not read token: {error}");
            return AuthFlowOutcome::Aborted;
        }
    };
    if entered.trim().is_empty() {
        match auth.clear_lm_studio_token() {
            Ok(true) => println!("Cleared the stored token; LM_API_TOKEN remains active."),
            Ok(false) => println!("Cleared any stored token; checking the server keylessly."),
            Err(error) => {
                println!("Error: could not clear stored token: {error}");
                return AuthFlowOutcome::Aborted;
            }
        }
        AuthFlowOutcome::RefreshOnly
    } else {
        println!("Captured an LM Studio API token; sending it to the runtime.");
        AuthFlowOutcome::StoredApiKey(entered)
    }
}

#[allow(clippy::print_stdout)]
fn api_key_flow(provider: &str) -> AuthFlowOutcome {
    println!("Entering API key for provider: {provider}");

    // Secure input via prompt_password
    let entered = match rpassword::prompt_password("API key (input hidden): ") {
        Ok(val) => val,
        Err(e) => {
            println!("Error: could not read key: {e}");
            String::new()
        }
    };
    if entered.trim().is_empty() {
        println!("Error: no key entered. Storing aborted.");
        return AuthFlowOutcome::Aborted;
    }
    println!("Successfully captured API key for '{provider}'! Sending to runtime...");
    AuthFlowOutcome::StoredApiKey(entered)
}

#[allow(clippy::print_stdout)]
fn report_oauth_outcome(result: Result<i64, String>) -> AuthFlowOutcome {
    match result {
        Ok(expires_at_unix) => {
            println!(
                "Stored a mjolnr-owned OAuth credential in an owner-only file.\n\
                 Access token expires at Unix time {expires_at_unix}; mjolnr refreshes it automatically."
            );
            AuthFlowOutcome::StoredOAuth
        }
        Err(error) => {
            println!("Login failed: {error}");
            AuthFlowOutcome::Aborted
        }
    }
}

/// Leave the alternate screen so ordinary stdin/stdout prompts work.
fn suspend_terminal() {
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    let _ = ratatui::try_restore();
}

/// Re-enter the alternate screen with the same flags the app started with.
fn restore_terminal() {
    let _ = crossterm::terminal::enable_raw_mode();
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::cursor::Hide,
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableMouseCapture
    );
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_prefers_the_draft_then_the_latest_failure() {
        let mut view = ViewState {
            composer: "draft".to_owned(),
            ..ViewState::default()
        };
        view.status = crate::tui::reducer::RunStatus::Failed {
            code: "PROVIDER_RELAY".to_owned(),
            detail: "gateway could not relay".to_owned(),
        };
        assert_eq!(clipboard_payload(&view).as_deref(), Some("draft"));

        view.composer.clear();
        assert_eq!(
            clipboard_payload(&view).as_deref(),
            Some("PROVIDER_RELAY: gateway could not relay")
        );
    }

    #[test]
    fn theme_file_round_trips_and_corruption_falls_back() {
        let directory =
            std::env::temp_dir().join(format!("mjolnr-theme-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&directory).expect("test directory");
        let path = directory.join("theme");

        std::fs::write(&path, "noir\n").expect("theme file");
        assert_eq!(read_theme(&path), Some(crate::tui::theme::ThemeId::Noir));

        std::fs::write(&path, "definitely-not-a-theme").expect("corrupt theme file");
        assert_eq!(read_theme(&path), None);

        std::fs::remove_dir_all(&directory).expect("test cleanup");
    }

    fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn a_closed_palette_claims_no_keys() {
        let mut view = ViewState::default();
        assert!(!view.jump_state.active);
        assert!(
            apply_jump_palette_key(key(crossterm::event::KeyCode::Char('a')), &mut view).is_none()
        );
        assert!(view.composer.is_empty());
    }

    #[test]
    fn an_open_palette_takes_typing_instead_of_the_composer() {
        let mut view = ViewState::default();
        view.jump_state.toggle();

        for character in "conv".chars() {
            assert!(
                apply_jump_palette_key(key(crossterm::event::KeyCode::Char(character)), &mut view)
                    .is_some()
            );
        }

        assert_eq!(view.jump_state.query, "conv");
        assert!(
            view.composer.is_empty(),
            "palette typing must never reach the composer"
        );
    }

    #[test]
    fn selecting_a_surface_navigates_and_closes_the_palette() {
        use crate::tui::workspace_types::WorkspaceSurface;

        let mut view = ViewState {
            active_surface: WorkspaceSurface::Work,
            ..ViewState::default()
        };
        view.jump_state.toggle();
        for character in "Verify".chars() {
            apply_jump_palette_key(key(crossterm::event::KeyCode::Char(character)), &mut view);
        }

        apply_jump_palette_key(key(crossterm::event::KeyCode::Enter), &mut view);

        assert_eq!(view.active_surface, WorkspaceSurface::Verify);
        assert!(!view.jump_state.active);
        assert!(view.jump_state.query.is_empty());
    }

    #[test]
    fn selecting_a_command_stages_it_rather_than_running_it() {
        let mut view = ViewState::default();
        view.jump_state.toggle();
        for character in "/model".chars() {
            apply_jump_palette_key(key(crossterm::event::KeyCode::Char(character)), &mut view);
        }

        apply_jump_palette_key(key(crossterm::event::KeyCode::Enter), &mut view);

        // Staged, not dispatched: the command still has to go through submit,
        // which is where every gate on it lives.
        assert_eq!(view.composer, "/model");
        assert!(!view.jump_state.active);
    }

    #[test]
    fn escape_closes_the_palette_without_navigating() {
        use crate::tui::workspace_types::WorkspaceSurface;

        let mut view = ViewState {
            active_surface: WorkspaceSurface::Work,
            ..ViewState::default()
        };
        view.jump_state.toggle();
        apply_jump_palette_key(key(crossterm::event::KeyCode::Char('p')), &mut view);
        apply_jump_palette_key(key(crossterm::event::KeyCode::Esc), &mut view);

        assert!(!view.jump_state.active);
        assert_eq!(view.active_surface, WorkspaceSurface::Work);
    }

    #[test]
    fn a_pending_approval_takes_the_screen_back_from_the_palette() {
        let mut view = ViewState::default();
        view.jump_state.toggle();
        view.snapshot.pending_approval = Some(crate::core::policy::PendingApproval {
            id: crate::core::command::ApprovalId::new(),
            tool_name: "run_command".to_owned(),
            tier: crate::core::tool::ToolTier::Execute,
            preview: "rm -rf build".to_owned(),
        });

        // The palette yields rather than swallowing the keys that answer the gate.
        assert!(
            apply_jump_palette_key(key(crossterm::event::KeyCode::Char('y')), &mut view).is_none()
        );
        assert!(!view.jump_state.active);
    }

    #[test]
    fn plan_approval_action_maps_to_the_authoritative_plan_revision() {
        let mut view = ViewState::default();
        let plan_id = crate::core::plan::PlanId::new();
        let revision_id = crate::core::plan::RevisionId::new(2);
        let mut workflow = crate::core::plan::PlanWorkflow::new(plan_id);
        workflow.active_revision = Some(crate::core::plan::RevisionId::new(1));
        workflow.stage = crate::core::plan::PlanStage::IterateRequested {
            proposal: crate::core::plan::PlanProposal {
                plan_id,
                revision_id: crate::core::plan::RevisionId::new(1),
                title: "Previous".to_owned(),
                summary: String::new(),
                steps: Vec::new(),
                proposed_at: time::OffsetDateTime::now_utc(),
            },
            feedback: "Revise".to_owned(),
        };
        workflow
            .propose_plan(crate::core::plan::PlanProposal {
                plan_id,
                revision_id,
                title: "Current".to_owned(),
                summary: String::new(),
                steps: Vec::new(),
                proposed_at: time::OffsetDateTime::now_utc(),
            })
            .unwrap();
        view.snapshot.plan = Some(workflow);

        let command = plan_approval_command(&view).expect("pending plan maps to approval");
        match command {
            MjolnrCommand::ApprovePlan { approval } => {
                assert_eq!(approval.plan_id, plan_id);
                assert_eq!(approval.revision_id, revision_id);
                assert_eq!(approval.decision, crate::core::plan::ReviewVerdict::Approve);
                assert_eq!(approval.approver, "Human");
            }
            other => panic!("expected plan approval command, got {other:?}"),
        }
    }

    #[test]
    fn plan_approval_action_refuses_non_pending_stages() {
        let mut view = ViewState::default();
        view.snapshot.plan = Some(crate::core::plan::PlanWorkflow::new(
            crate::core::plan::PlanId::new(),
        ));

        assert!(plan_approval_command(&view).is_none());
    }

    #[test]
    fn surface_navigation_is_reversible() {
        use crate::tui::workspace_types::WorkspaceSurface;

        let mut view = ViewState {
            active_surface: WorkspaceSurface::Work,
            ..ViewState::default()
        };

        view.next_surface();
        assert_eq!(view.active_surface, WorkspaceSurface::Conversation);
        view.previous_surface();
        assert_eq!(view.active_surface, WorkspaceSurface::Work);

        view.jump_to_attention();
        assert_eq!(view.active_surface, WorkspaceSurface::Attention);
    }
}
