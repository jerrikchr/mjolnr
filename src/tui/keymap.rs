//! Context-aware terminal key resolution.
//!
//! Physical keys become semantic actions here. The event loop applies those
//! actions; it does not grow an order-dependent pile of raw-key branches.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::core::command::ApprovalDecision;
use crate::core::recovery::RecoveryDecision;
use crate::core::tool::ToolTier;

const EXIT_CONFIRMATION_WINDOW: Duration = Duration::from_millis(750);

/// What the UI is currently showing, as far as key resolution cares.
///
/// The lint targets bools used as an implicit state machine. These are
/// independent facts about one moment — a run can be active while the composer
/// is empty and help is open — and they are read, never sequenced. Enums here
/// would be `RunActive::Yes | RunActive::No`, which is worse. The same
/// reasoning, and the same exception, as `core::model::ModelCapabilities`.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts about one moment, not a state machine"
)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct InputContext {
    pub run_active: bool,
    pub composer_empty: bool,
    pub help_open: bool,
    pub approval_tier: Option<ToolTier>,
    /// A crash left work mjolnr cannot account for. Takes precedence over every
    /// other context: nothing else may be typed until it is resolved.
    pub recovery_required: bool,
    /// The model picker is open, so arrows and Enter drive the list rather than
    /// the composer.
    pub picker_open: bool,
    /// The slash-command menu is showing, so Tab completes rather than doing
    /// nothing.
    pub command_menu_open: bool,
    pub plan_approval_pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputAction {
    Continue,
    Redraw,
    Quit,
    Interrupt,
    ArmExit,
    ClearComposer,
    Submit,
    Newline,
    DeleteBackward,
    Insert(char),
    CyclePolicy,
    ToggleHelp,
    ToggleToolDetails,
    ScrollUp,
    ScrollDown,
    ScrollBottom,
    ResolveApproval(ApprovalDecision),
    ResolveRecovery(RecoveryDecision),
    PickerUp,
    PickerDown,
    PickerConfirm,
    PickerCancel,
    CompleteCommand,
    MoveCursorLeft,
    MoveCursorRight,
    MoveCursorWordLeft,
    MoveCursorWordRight,
    MoveToLineStart,
    MoveToLineEnd,
    DeleteForward,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteToLineStart,
    DeleteToLineEnd,
    EditExternally,
    CopyClipboard,
    PasteClipboard,
    ApprovePlan,
    QueueComposer,
    /// Send the composer as a steering message for the run in flight (plan
    /// §Phase 16.5). Distinct from `QueueComposer`, which waits for the run to
    /// finish: steering redirects work already underway.
    SteerComposer,
    RecallQueuedComposer,
    ToggleJumpPalette,
    JumpAttention,
    NextSurface,
    PreviousSurface,
    SelectSurface(crate::tui::workspace_types::WorkspaceSurface),
}

#[derive(Debug, Default)]
pub(crate) struct KeymapState {
    exit_armed_at: Option<Instant>,
}

impl KeymapState {
    #[must_use]
    pub(crate) const fn exit_armed(&self) -> bool {
        self.exit_armed_at.is_some()
    }

    /// Expire the double-press window. Returns true when visible state changed.
    pub(crate) fn expire(&mut self, now: Instant) -> bool {
        let expired = self
            .exit_armed_at
            .is_some_and(|armed| now.saturating_duration_since(armed) >= EXIT_CONFIRMATION_WINDOW);
        if expired {
            self.exit_armed_at = None;
        }
        expired
    }

    pub(crate) fn disarm(&mut self) {
        self.exit_armed_at = None;
    }

    pub(crate) fn resolve(
        &mut self,
        key: KeyEvent,
        context: InputContext,
        now: Instant,
    ) -> InputAction {
        let ctrl_c = control_char(key, 'c');
        self.expire(now);
        if !ctrl_c {
            self.disarm();
        }

        // Recovery outranks everything. A session whose history mjolnr cannot
        // account for must not accept a prompt, an approval, or a policy change
        // — the guard is here, in the one place physical keys become intent,
        // rather than spread across the branches below.
        if context.recovery_required && !context.help_open {
            return self.resolve_recovery(key, context, now);
        }
        if let Some(tier) = context.approval_tier {
            return self.resolve_approval(key, tier, context, now);
        }
        if context.help_open {
            return self.resolve_help(key, context, now);
        }
        if context.picker_open {
            return self.resolve_picker(key, context, now);
        }
        self.resolve_main(key, context, now)
    }

    /// Resolve the model picker.
    ///
    /// Typing stays live so the list can be filtered, so only the navigation
    /// keys are claimed. Esc cancels without selecting — opening a picker must
    /// never be a commitment.
    fn resolve_picker(
        &mut self,
        key: KeyEvent,
        context: InputContext,
        now: Instant,
    ) -> InputAction {
        match key.code {
            KeyCode::Up => InputAction::PickerUp,
            KeyCode::Down => InputAction::PickerDown,
            KeyCode::Enter => InputAction::PickerConfirm,
            KeyCode::Esc => InputAction::PickerCancel,
            KeyCode::Backspace => InputAction::DeleteBackward,
            KeyCode::Char(character) if text_modifiers_only(key) => InputAction::Insert(character),
            _ if control_char(key, 'c') => self.resolve_ctrl_c(context, now),
            _ => InputAction::Continue,
        }
    }

    /// Resolve the recovery gate.
    ///
    /// The keys are deliberately **not** the approval modal's `y`/`n`/`a`. An
    /// operator who has answered a hundred approvals has `y` in their fingers,
    /// and the two questions are not alike: an approval authorises work that has
    /// not started, while this decides what to do about work that may already
    /// have changed the repository. A slip here is not recoverable by the next
    /// prompt.
    fn resolve_recovery(
        &mut self,
        key: KeyEvent,
        context: InputContext,
        now: Instant,
    ) -> InputAction {
        match lower_character(key) {
            Some('c') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                InputAction::ResolveRecovery(RecoveryDecision::AbandonAndContinue)
            }
            Some('e') => InputAction::ResolveRecovery(RecoveryDecision::EndSession),
            Some('?') => InputAction::ToggleHelp,
            _ if control_char(key, 'c') => self.resolve_ctrl_c(context, now),
            _ if control_char(key, 'd') => InputAction::Quit,
            _ => InputAction::Continue,
        }
    }

    fn resolve_approval(
        &mut self,
        key: KeyEvent,
        tier: ToolTier,
        context: InputContext,
        now: Instant,
    ) -> InputAction {
        match lower_character(key) {
            Some('y') => InputAction::ResolveApproval(ApprovalDecision::ApproveOnce),
            Some('n') => InputAction::ResolveApproval(ApprovalDecision::Deny),
            Some('a') if tier == ToolTier::Execute => {
                InputAction::ResolveApproval(ApprovalDecision::ApproveExactForSession)
            }
            _ if key.code == KeyCode::Esc => InputAction::Interrupt,
            _ if control_char(key, 'c') => self.resolve_ctrl_c(context, now),
            _ => InputAction::Continue,
        }
    }

    fn resolve_help(&mut self, key: KeyEvent, context: InputContext, now: Instant) -> InputAction {
        if key.code == KeyCode::Esc || key.code == KeyCode::F(1) || key.code == KeyCode::Char('?') {
            InputAction::ToggleHelp
        } else if control_char(key, 'c') {
            self.resolve_ctrl_c(context, now)
        } else {
            InputAction::Continue
        }
    }

    fn resolve_cursor_keys(key: KeyEvent) -> Option<InputAction> {
        match key.code {
            KeyCode::Left
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Some(InputAction::MoveCursorWordLeft)
            }
            KeyCode::Left => Some(InputAction::MoveCursorLeft),
            KeyCode::Right
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Some(InputAction::MoveCursorWordRight)
            }
            KeyCode::Right => Some(InputAction::MoveCursorRight),
            KeyCode::Home => Some(InputAction::MoveToLineStart),
            KeyCode::End if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::MoveToLineEnd)
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::MoveToLineStart)
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::MoveToLineEnd)
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::MoveCursorRight)
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::MoveCursorLeft)
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
                Some(InputAction::MoveCursorWordRight)
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => {
                Some(InputAction::MoveCursorWordLeft)
            }
            _ => None,
        }
    }

    fn resolve_deletion_keys(key: KeyEvent) -> Option<InputAction> {
        match key.code {
            KeyCode::Backspace
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Some(InputAction::DeleteWordBackward)
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::DeleteWordBackward)
            }
            KeyCode::Delete
                if key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Some(InputAction::DeleteWordForward)
            }
            KeyCode::Delete => Some(InputAction::DeleteForward),
            KeyCode::Backspace => Some(InputAction::DeleteBackward),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::DeleteToLineStart)
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::DeleteToLineEnd)
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::ALT) => {
                Some(InputAction::DeleteWordForward)
            }
            _ => None,
        }
    }

    /// Workspace navigation, resolved **before** the text editor.
    ///
    /// Order is load-bearing and every binding here is guarded to earn its
    /// place ahead of editing. The phase that introduced these actions bound
    /// them to `Alt+Left`/`Alt+Right`/`Ctrl+J`/`Ctrl+A` and placed this
    /// resolver *after* `resolve_text_editor_keys`, where word-movement,
    /// newline, and line-start had already claimed all four: the actions
    /// resolved from no key a user could actually press.
    ///
    /// - `Ctrl+PageUp`/`Ctrl+PageDown` are not editing keys in any mode.
    /// - `Ctrl+P` is unbound elsewhere.
    /// - `Ctrl+A` yields to line-start the moment there is text to move
    ///   through; on an empty composer, moving to the start of nothing is the
    ///   weaker claim.
    ///
    /// `Ctrl+J` deliberately stays `Newline`: it is the newline that survives
    /// terminals which cannot report `Shift+Enter`, so the palette does not
    /// get to take it.
    fn resolve_workspace_navigation_keys(
        key: KeyEvent,
        context: InputContext,
    ) -> Option<InputAction> {
        let has_mod = key.modifiers.contains(KeyModifiers::ALT)
            || key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Char('1') if has_mod => Some(InputAction::SelectSurface(
                crate::tui::workspace_types::WorkspaceSurface::Work,
            )),
            KeyCode::Char('2') if has_mod => Some(InputAction::SelectSurface(
                crate::tui::workspace_types::WorkspaceSurface::Conversation,
            )),
            KeyCode::Char('3') if has_mod => Some(InputAction::SelectSurface(
                crate::tui::workspace_types::WorkspaceSurface::Plan,
            )),
            KeyCode::Char('4') if has_mod => Some(InputAction::SelectSurface(
                crate::tui::workspace_types::WorkspaceSurface::Changes,
            )),
            KeyCode::Char('5') if has_mod => Some(InputAction::SelectSurface(
                crate::tui::workspace_types::WorkspaceSurface::Verify,
            )),
            KeyCode::Char('6') if has_mod => Some(InputAction::SelectSurface(
                crate::tui::workspace_types::WorkspaceSurface::Attention,
            )),
            KeyCode::PageDown if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::NextSurface)
            }
            KeyCode::PageUp if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::PreviousSurface)
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::ToggleJumpPalette)
            }
            KeyCode::Char('a')
                if key.modifiers.contains(KeyModifiers::CONTROL) && context.composer_empty =>
            {
                Some(InputAction::JumpAttention)
            }
            _ => None,
        }
    }

    fn resolve_text_editor_keys(key: KeyEvent, context: InputContext) -> Option<InputAction> {
        match key.code {
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
            {
                Some(InputAction::Newline)
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) && context.run_active => {
                Some(InputAction::QueueComposer)
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                Some(InputAction::Newline)
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                Some(InputAction::RecallQueuedComposer)
            }
            KeyCode::Enter if context.run_active => Some(InputAction::SteerComposer),
            KeyCode::Enter => Some(InputAction::Submit),
            KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::EditExternally)
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::PasteClipboard)
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::SUPER) => {
                Some(InputAction::PasteClipboard)
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::SUPER) => {
                Some(InputAction::CopyClipboard)
            }
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::CopyClipboard)
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(InputAction::Newline)
            }
            _ => {
                if let Some(character) = resolve_inserted_character(key) {
                    Some(InputAction::Insert(character))
                } else if let Some(action) = Self::resolve_cursor_keys(key) {
                    Some(action)
                } else {
                    Self::resolve_deletion_keys(key)
                }
            }
        }
    }

    fn resolve_main(&mut self, key: KeyEvent, context: InputContext, now: Instant) -> InputAction {
        if control_char(key, 'c') {
            return self.resolve_ctrl_c(context, now);
        }
        if control_char(key, 'd') {
            return Self::resolve_ctrl_d(context);
        }

        // Plain Tab completes only while the command menu is up. Elsewhere it
        // stays unbound rather than becoming a second policy cycle: the policy
        // key is deliberately SHIFT-TAB, and a near-miss that silently widened
        // policy is exactly the accident worth avoiding. Resolved before the
        // match so the SHIFT-TAB arms below keep reading as one pair.
        if key.code == KeyCode::Tab
            && context.command_menu_open
            && !key.modifiers.contains(KeyModifiers::SHIFT)
        {
            return InputAction::CompleteCommand;
        }

        if let Some(action) = Self::resolve_plan_approval(key, context) {
            return action;
        }

        if let Some(action) = Self::resolve_workspace_navigation_keys(key, context) {
            return action;
        }

        if let Some(action) = Self::resolve_text_editor_keys(key, context) {
            return action;
        }

        match key.code {
            KeyCode::Esc if context.run_active => InputAction::Interrupt,
            KeyCode::BackTab if !context.run_active => InputAction::CyclePolicy,
            KeyCode::Tab if !context.run_active && key.modifiers.contains(KeyModifiers::SHIFT) => {
                InputAction::CyclePolicy
            }
            KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                InputAction::ScrollBottom
            }
            KeyCode::PageUp => InputAction::ScrollUp,
            KeyCode::PageDown => InputAction::ScrollDown,
            KeyCode::Up
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
            {
                InputAction::ScrollUp
            }
            KeyCode::Down
                if key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
            {
                InputAction::ScrollDown
            }
            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                InputAction::Redraw
            }
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                InputAction::ToggleToolDetails
            }
            KeyCode::F(1) => InputAction::ToggleHelp,
            _ => {
                if let Some(character) = resolve_inserted_character(key) {
                    InputAction::Insert(character)
                } else {
                    InputAction::Continue
                }
            }
        }
    }

    fn resolve_ctrl_d(context: InputContext) -> InputAction {
        if context.run_active {
            InputAction::Interrupt
        } else if context.composer_empty {
            InputAction::Quit
        } else {
            InputAction::Continue
        }
    }

    fn resolve_plan_approval(key: KeyEvent, context: InputContext) -> Option<InputAction> {
        if context.plan_approval_pending {
            if control_char(key, 'y') {
                return Some(InputAction::ApprovePlan);
            }
            if context.composer_empty && lower_character(key) == Some('y') {
                return Some(InputAction::ApprovePlan);
            }
        }
        None
    }

    fn resolve_ctrl_c(&mut self, context: InputContext, now: Instant) -> InputAction {
        if self.exit_armed_at.is_some() {
            self.exit_armed_at = None;
            return InputAction::Quit;
        }

        self.exit_armed_at = Some(now);
        if context.run_active || context.approval_tier.is_some() {
            InputAction::Interrupt
        } else if !context.composer_empty {
            InputAction::ClearComposer
        } else {
            InputAction::ArmExit
        }
    }
}

fn resolve_inserted_character(key: KeyEvent) -> Option<char> {
    if let KeyCode::Char(c) = key.code
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::SUPER)
    {
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            return Some(shifted_ascii_character(c));
        }
        return Some(c);
    }
    None
}

fn shifted_ascii_character(character: char) -> char {
    match character {
        '`' => '~',
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        other => other.to_ascii_uppercase(),
    }
}

fn control_char(key: KeyEvent, character: char) -> bool {
    key.code == KeyCode::Char(character) && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn lower_character(key: KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(character) if text_modifiers_only(key) => {
            Some(character.to_ascii_lowercase())
        }
        _ => None,
    }
}

fn text_modifiers_only(key: KeyEvent) -> bool {
    if let KeyCode::Char(c) = key.code
        && c.is_ascii_uppercase()
    {
        return !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
    }
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn idle(composer_empty: bool) -> InputContext {
        InputContext {
            run_active: false,
            composer_empty,
            help_open: false,
            approval_tier: None,
            recovery_required: false,
            picker_open: false,
            command_menu_open: false,
            plan_approval_pending: false,
        }
    }

    #[test]
    fn ctrl_c_requires_a_second_press_inside_the_window() {
        let mut state = KeymapState::default();
        let start = Instant::now();
        let ctrl_c = key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        assert_eq!(
            state.resolve(ctrl_c, idle(true), start),
            InputAction::ArmExit
        );
        assert!(state.exit_armed());
        assert_eq!(
            state.resolve(ctrl_c, idle(true), start + Duration::from_millis(500)),
            InputAction::Quit
        );
    }

    #[test]
    fn expired_ctrl_c_is_a_new_first_press() {
        let mut state = KeymapState::default();
        let start = Instant::now();
        let ctrl_c = key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        assert_eq!(
            state.resolve(ctrl_c, idle(true), start),
            InputAction::ArmExit
        );
        assert_eq!(
            state.resolve(ctrl_c, idle(true), start + Duration::from_secs(1)),
            InputAction::ArmExit
        );
    }

    #[test]
    fn first_ctrl_c_interrupts_active_work_instead_of_quitting() {
        let mut state = KeymapState::default();
        let context = InputContext {
            run_active: true,
            ..idle(true)
        };
        let action = state.resolve(
            key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            context,
            Instant::now(),
        );

        assert_eq!(action, InputAction::Interrupt);
        assert!(state.exit_armed());
    }

    #[test]
    fn first_ctrl_c_clears_idle_input_and_arms_exit() {
        let mut state = KeymapState::default();
        let action = state.resolve(
            key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            idle(false),
            Instant::now(),
        );

        assert_eq!(action, InputAction::ClearComposer);
        assert!(state.exit_armed());
    }

    #[test]
    fn ctrl_d_only_quits_from_an_empty_idle_prompt() {
        let mut state = KeymapState::default();
        let ctrl_d = key(KeyCode::Char('d'), KeyModifiers::CONTROL);

        assert_eq!(
            state.resolve(ctrl_d, idle(true), Instant::now()),
            InputAction::Quit
        );
        assert_eq!(
            state.resolve(ctrl_d, idle(false), Instant::now()),
            InputAction::Continue
        );
    }

    #[test]
    fn policy_cycling_is_locked_during_a_run() {
        let mut state = KeymapState::default();
        let mut context = idle(true);
        let shift_tab = key(KeyCode::BackTab, KeyModifiers::SHIFT);

        assert_eq!(
            state.resolve(shift_tab, context, Instant::now()),
            InputAction::CyclePolicy
        );
        context.run_active = true;
        assert_eq!(
            state.resolve(shift_tab, context, Instant::now()),
            InputAction::Continue
        );
    }

    #[test]
    fn enter_steers_during_a_run_and_multiline_editing_remains_available() {
        let mut state = KeymapState::default();
        let mut context = idle(false);
        context.run_active = true;

        assert_eq!(
            state.resolve(
                key(KeyCode::Enter, KeyModifiers::NONE),
                context,
                Instant::now()
            ),
            // : Enter during a run steers it rather than doing
            // nothing. Alt+Enter still queues a follow-up for after it ends.
            InputAction::SteerComposer
        );
        assert_eq!(
            state.resolve(
                key(KeyCode::Enter, KeyModifiers::SHIFT),
                context,
                Instant::now()
            ),
            InputAction::Newline
        );
    }

    #[test]
    fn approval_shortcuts_preserve_the_exact_command_boundary() {
        let mut state = KeymapState::default();
        let now = Instant::now();
        let mut context = idle(true);
        context.approval_tier = Some(ToolTier::Write);

        assert_eq!(
            state.resolve(key(KeyCode::Char('y'), KeyModifiers::NONE), context, now),
            InputAction::ResolveApproval(ApprovalDecision::ApproveOnce)
        );
        assert_eq!(
            state.resolve(key(KeyCode::Char('a'), KeyModifiers::NONE), context, now),
            InputAction::Continue
        );

        context.approval_tier = Some(ToolTier::Execute);
        assert_eq!(
            state.resolve(key(KeyCode::Char('a'), KeyModifiers::NONE), context, now),
            InputAction::ResolveApproval(ApprovalDecision::ApproveExactForSession)
        );
    }

    #[test]
    fn control_shortcuts_never_insert_their_letter() {
        let mut state = KeymapState::default();
        // 'z' stands in for any unbound control chord: the property under test
        // is that an unhandled Ctrl+letter is swallowed rather than typed.
        let action = state.resolve(
            key(KeyCode::Char('z'), KeyModifiers::CONTROL),
            idle(false),
            Instant::now(),
        );
        assert_eq!(action, InputAction::Continue);
    }

    #[test]
    fn workspace_navigation_resolves_from_keys_the_editor_does_not_claim() {
        let mut state = KeymapState::default();
        let now = Instant::now();

        assert_eq!(
            state.resolve(
                key(KeyCode::PageDown, KeyModifiers::CONTROL),
                idle(false),
                now
            ),
            InputAction::NextSurface
        );
        assert_eq!(
            state.resolve(
                key(KeyCode::PageUp, KeyModifiers::CONTROL),
                idle(false),
                now
            ),
            InputAction::PreviousSurface
        );
        assert_eq!(
            state.resolve(
                key(KeyCode::Char('p'), KeyModifiers::CONTROL),
                idle(false),
                now
            ),
            InputAction::ToggleJumpPalette
        );
    }

    #[test]
    fn plain_paging_still_scrolls_the_transcript() {
        let mut state = KeymapState::default();
        let now = Instant::now();

        assert_eq!(
            state.resolve(key(KeyCode::PageUp, KeyModifiers::NONE), idle(false), now),
            InputAction::ScrollUp
        );
        assert_eq!(
            state.resolve(key(KeyCode::PageDown, KeyModifiers::NONE), idle(false), now),
            InputAction::ScrollDown
        );
    }

    #[test]
    fn workspace_navigation_never_shadows_composer_editing() {
        let mut state = KeymapState::default();
        let now = Instant::now();

        // Ctrl+J stays the newline that works where Shift+Enter cannot be reported.
        assert_eq!(
            state.resolve(
                key(KeyCode::Char('j'), KeyModifiers::CONTROL),
                idle(false),
                now
            ),
            InputAction::Newline
        );
        // Word movement keeps Alt+Left / Alt+Right.
        assert_eq!(
            state.resolve(key(KeyCode::Left, KeyModifiers::ALT), idle(false), now),
            InputAction::MoveCursorWordLeft
        );
        assert_eq!(
            state.resolve(key(KeyCode::Right, KeyModifiers::ALT), idle(false), now),
            InputAction::MoveCursorWordRight
        );
    }

    #[test]
    fn control_a_jumps_to_attention_only_when_there_is_no_line_to_start() {
        let mut state = KeymapState::default();
        let now = Instant::now();

        assert_eq!(
            state.resolve(
                key(KeyCode::Char('a'), KeyModifiers::CONTROL),
                idle(false),
                now
            ),
            InputAction::MoveToLineStart
        );
        assert_eq!(
            state.resolve(
                key(KeyCode::Char('a'), KeyModifiers::CONTROL),
                idle(true),
                now
            ),
            InputAction::JumpAttention
        );
    }

    #[test]
    fn recovery_blocks_typing_and_offers_only_its_own_choices() {
        // A session mjolnr cannot account for must not accept a directive. The
        // guard lives in the keymap so no client can forget it.
        let mut state = KeymapState::default();
        let context = InputContext {
            recovery_required: true,
            ..idle(true)
        };

        assert_eq!(
            state.resolve(
                key(KeyCode::Char('x'), KeyModifiers::NONE),
                context,
                Instant::now()
            ),
            InputAction::Continue,
            "a halted session must not accept typed text"
        );
        assert_eq!(
            state.resolve(
                key(KeyCode::Enter, KeyModifiers::NONE),
                context,
                Instant::now()
            ),
            InputAction::Continue,
            "a halted session must not submit a directive"
        );
        assert_eq!(
            state.resolve(
                key(KeyCode::BackTab, KeyModifiers::NONE),
                context,
                Instant::now()
            ),
            InputAction::Continue,
            "a halted session must not change policy"
        );

        assert_eq!(
            state.resolve(
                key(KeyCode::Char('c'), KeyModifiers::NONE),
                context,
                Instant::now()
            ),
            InputAction::ResolveRecovery(RecoveryDecision::AbandonAndContinue)
        );
        assert_eq!(
            state.resolve(
                key(KeyCode::Char('e'), KeyModifiers::NONE),
                context,
                Instant::now()
            ),
            InputAction::ResolveRecovery(RecoveryDecision::EndSession)
        );
    }

    #[test]
    fn recovery_does_not_reuse_the_approval_keys() {
        // `y` is in an operator's fingers after a hundred approvals. The two
        // questions are not alike, and a slip on this one is not recoverable by
        // the next prompt.
        let mut state = KeymapState::default();
        let context = InputContext {
            recovery_required: true,
            ..idle(true)
        };

        for character in ['y', 'n', 'a'] {
            assert_eq!(
                state.resolve(
                    key(KeyCode::Char(character), KeyModifiers::NONE),
                    context,
                    Instant::now()
                ),
                InputAction::Continue,
                "`{character}` must not resolve a recovery: it means approval elsewhere"
            );
        }
    }

    #[test]
    fn uppercase_characters_with_shift_or_none_resolve_to_insert() {
        let mut state = KeymapState::default();
        let context = idle(false);
        let now = Instant::now();

        assert_eq!(
            state.resolve(key(KeyCode::Char('A'), KeyModifiers::SHIFT), context, now),
            InputAction::Insert('A')
        );
        assert_eq!(
            state.resolve(key(KeyCode::Char('a'), KeyModifiers::SHIFT), context, now),
            InputAction::Insert('A')
        );
        assert_eq!(
            state.resolve(key(KeyCode::Char('Z'), KeyModifiers::NONE), context, now),
            InputAction::Insert('Z')
        );
    }

    #[test]
    fn shifted_ascii_punctuation_resolves_to_the_printed_symbol() {
        let mut state = KeymapState::default();
        let context = idle(false);
        let now = Instant::now();
        let pairs = [
            ('`', '~'),
            ('1', '!'),
            ('2', '@'),
            ('3', '#'),
            ('4', '$'),
            ('5', '%'),
            ('6', '^'),
            ('7', '&'),
            ('8', '*'),
            ('9', '('),
            ('0', ')'),
            ('-', '_'),
            ('=', '+'),
            ('[', '{'),
            (']', '}'),
            ('\\', '|'),
            (';', ':'),
            ('\'', '"'),
            (',', '<'),
            ('.', '>'),
            ('/', '?'),
        ];

        for (unshifted, shifted) in pairs {
            assert_eq!(
                state.resolve(
                    key(KeyCode::Char(unshifted), KeyModifiers::SHIFT),
                    context,
                    now
                ),
                InputAction::Insert(shifted),
                "Shift+{unshifted} should insert {shifted}"
            );
        }
    }

    #[test]
    fn recovery_outranks_a_pending_approval() {
        // Both cannot normally be true at once, but if they were, the honest
        // thing to answer is the question about what already happened.
        let mut state = KeymapState::default();
        let context = InputContext {
            recovery_required: true,
            approval_tier: Some(ToolTier::Execute),
            ..idle(true)
        };

        assert_eq!(
            state.resolve(
                key(KeyCode::Char('y'), KeyModifiers::NONE),
                context,
                Instant::now()
            ),
            InputAction::Continue,
            "the approval modal must not be reachable while recovery is pending"
        );
        assert_eq!(
            state.resolve(
                key(KeyCode::Char('c'), KeyModifiers::NONE),
                context,
                Instant::now()
            ),
            InputAction::ResolveRecovery(RecoveryDecision::AbandonAndContinue)
        );
    }

    #[test]
    fn a_halted_session_can_still_be_left() {
        // Trapping a user in a modal they cannot exit would be its own bug.
        let mut state = KeymapState::default();
        let context = InputContext {
            recovery_required: true,
            ..idle(true)
        };

        assert_eq!(
            state.resolve(
                key(KeyCode::Char('d'), KeyModifiers::CONTROL),
                context,
                Instant::now()
            ),
            InputAction::Quit
        );

        let mut state = KeymapState::default();
        assert_eq!(
            state.resolve(
                key(KeyCode::Char('c'), KeyModifiers::CONTROL),
                context,
                Instant::now()
            ),
            InputAction::ArmExit,
            "ctrl-c must keep its documented double-press meaning here too"
        );
        assert_eq!(
            state.resolve(
                key(KeyCode::Char('c'), KeyModifiers::CONTROL),
                context,
                Instant::now()
            ),
            InputAction::Quit
        );
    }

    #[test]
    fn the_help_panel_is_reachable_from_a_halted_session() {
        let mut state = KeymapState::default();
        let context = InputContext {
            recovery_required: true,
            ..idle(true)
        };
        assert_eq!(
            state.resolve(
                key(KeyCode::Char('?'), KeyModifiers::NONE),
                context,
                Instant::now()
            ),
            InputAction::ToggleHelp
        );
    }

    #[test]
    fn question_mark_is_ordinary_text_in_an_empty_composer() {
        let mut state = KeymapState::default();
        assert_eq!(
            state.resolve(
                key(KeyCode::Char('?'), KeyModifiers::NONE),
                idle(true),
                Instant::now()
            ),
            InputAction::Insert('?')
        );
    }

    #[test]
    fn f1_opens_the_keymap_without_claiming_text_punctuation() {
        let mut state = KeymapState::default();
        assert_eq!(
            state.resolve(
                key(KeyCode::F(1), KeyModifiers::NONE),
                idle(true),
                Instant::now()
            ),
            InputAction::ToggleHelp
        );
    }

    #[test]
    fn text_editor_shortcuts_resolve_correctly() {
        let mut state = KeymapState::default();
        let context = idle(false);
        let now = Instant::now();

        let test_cases = [
            (
                key(KeyCode::Left, KeyModifiers::NONE),
                InputAction::MoveCursorLeft,
            ),
            (
                key(KeyCode::Right, KeyModifiers::NONE),
                InputAction::MoveCursorRight,
            ),
            (
                key(KeyCode::Left, KeyModifiers::CONTROL),
                InputAction::MoveCursorWordLeft,
            ),
            (
                key(KeyCode::Right, KeyModifiers::CONTROL),
                InputAction::MoveCursorWordRight,
            ),
            (
                key(KeyCode::Home, KeyModifiers::NONE),
                InputAction::MoveToLineStart,
            ),
            (
                key(KeyCode::End, KeyModifiers::NONE),
                InputAction::MoveToLineEnd,
            ),
            (
                key(KeyCode::Char('a'), KeyModifiers::CONTROL),
                InputAction::MoveToLineStart,
            ),
            (
                key(KeyCode::Char('e'), KeyModifiers::CONTROL),
                InputAction::MoveToLineEnd,
            ),
            (
                key(KeyCode::Char('g'), KeyModifiers::CONTROL),
                InputAction::EditExternally,
            ),
            (
                key(KeyCode::Char('v'), KeyModifiers::CONTROL),
                InputAction::PasteClipboard,
            ),
            (
                key(KeyCode::Char('v'), KeyModifiers::SUPER),
                InputAction::PasteClipboard,
            ),
            (
                key(KeyCode::Char('c'), KeyModifiers::SUPER),
                InputAction::CopyClipboard,
            ),
            (
                key(KeyCode::Char('y'), KeyModifiers::CONTROL),
                InputAction::CopyClipboard,
            ),
            (
                key(KeyCode::Backspace, KeyModifiers::NONE),
                InputAction::DeleteBackward,
            ),
            (
                key(KeyCode::Backspace, KeyModifiers::CONTROL),
                InputAction::DeleteWordBackward,
            ),
            (
                key(KeyCode::Char('w'), KeyModifiers::CONTROL),
                InputAction::DeleteWordBackward,
            ),
            (
                key(KeyCode::Delete, KeyModifiers::NONE),
                InputAction::DeleteForward,
            ),
            (
                key(KeyCode::Delete, KeyModifiers::CONTROL),
                InputAction::DeleteWordForward,
            ),
            (
                key(KeyCode::Char('u'), KeyModifiers::CONTROL),
                InputAction::DeleteToLineStart,
            ),
            (
                key(KeyCode::Char('k'), KeyModifiers::CONTROL),
                InputAction::DeleteToLineEnd,
            ),
            (
                key(KeyCode::End, KeyModifiers::CONTROL),
                InputAction::ScrollBottom,
            ),
        ];

        for (k, expected) in test_cases {
            assert_eq!(state.resolve(k, context, now), expected);
        }
    }

    #[test]
    fn plan_approval_shortcuts_resolve_correctly() {
        let mut state = KeymapState::default();
        let context = InputContext {
            plan_approval_pending: true,
            composer_empty: true,
            ..idle(true)
        };
        let now = Instant::now();

        // 'y' when composer is empty resolves to ApprovePlan
        assert_eq!(
            state.resolve(key(KeyCode::Char('y'), KeyModifiers::NONE), context, now),
            InputAction::ApprovePlan
        );

        // Ctrl+Y resolves to ApprovePlan
        assert_eq!(
            state.resolve(key(KeyCode::Char('y'), KeyModifiers::CONTROL), context, now),
            InputAction::ApprovePlan
        );

        // When composer is not empty, 'y' does NOT resolve to ApprovePlan
        let context_not_empty = InputContext {
            composer_empty: false,
            ..context
        };
        assert_ne!(
            state.resolve(
                key(KeyCode::Char('y'), KeyModifiers::NONE),
                context_not_empty,
                now
            ),
            InputAction::ApprovePlan
        );
    }
}
