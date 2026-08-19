//! Provider-neutral project-context values.
//!
//! These are inert descriptions, never authority. Project prose and skill
//! instructions may influence a model, but every resulting side effect still
//! crosses smed's normal deterministic tool and policy gates.

use crate::core::error::ReasonCode;

/// Where a discovered skill came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillScope {
    Project,
    User,
}

impl SkillScope {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

/// The progressive-disclosure catalog entry visible before activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub location: String,
    pub scope: SkillScope,
}

/// A discovered agent-authored extension, before it is loaded.
///
/// Discovery makes an extension *visible* — listed here, reportable by
/// `/reload`. It does not make it *callable*: that is the separate, evidenced
/// load act. The shape mirrors [`SkillSummary`] deliberately; the difference
/// that matters is the two-step visible-then-loaded lifecycle, not the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionSummary {
    pub name: String,
    pub description: String,
    pub location: String,
    pub scope: SkillScope,
}

/// A prompt template as a client renders it.
///
/// The body is deliberately absent: a client shows the catalogue and asks the
/// runtime to expand, so template text never has to be kept in sync in two
/// places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSummary {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub scope: SkillScope,
}

/// A discovered persona as a client renders it.
///
/// The body is deliberately absent, exactly like [`PromptSummary`]: a client
/// shows the name and description and asks the runtime to overlay by name, so
/// persona prose never has to be kept in sync in two places. Inert: selecting a
/// persona changes voice, never capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaSummary {
    pub name: String,
    pub description: Option<String>,
    pub scope: SkillScope,
}

/// What a `/reload` found.
///
/// Carried on the snapshot rather than the event log: every durable event
/// belongs to a session, and a reload is legitimate before one is open. It is
/// also a statement about files on disk, which are already under version
/// control — the log would be recording a census of something the filesystem
/// owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadReport {
    pub skills: usize,
    pub prompts: usize,
    /// What appeared or vanished, so the acknowledgement says what happened
    /// rather than only that something did. Empty means "nothing changed",
    /// which is a useful answer and not the same as "no reload ran".
    pub changes: Vec<String>,
    /// Set when the reload itself failed; the previous resources stay live.
    pub failure: Option<String>,
}

/// The result of a load-extension act, surfaced on the snapshot.
///
/// Carried the same way [`ReloadReport`] is: on the snapshot, not the event log
/// on failure. A *successful* load is a durable `ExtensionLoaded` event; this
/// report is the client-facing acknowledgement of either outcome, so a refused
/// load — which writes nothing — still has somewhere to be reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionLoadReport {
    pub name: String,
    /// The program the loaded extension runs, on success.
    pub loaded_program: Option<String>,
    /// Set when the load was refused; nothing was registered.
    pub failure: Option<String>,
}

/// A classifiable discovery or validation problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextDiagnostic {
    pub code: ReasonCode,
    pub detail: String,
}
