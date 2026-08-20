//! What mjolnr is, told to the model that runs inside it.
//!
//! Distinct from [`super::self_docs`], which points at this repository's own
//! contracts for when the task is *extending mjolnr*. This module answers a
//! question every session faces in any repository: what harness am I running
//! in, and what does it enforce on me? Without it the model knows its tool
//! schemas and nothing about the machine holding them — so "what can you do?"
//! has no answer in context, and the only way to look is the filesystem.
//!
//! Written as **facts, not rules**. A rule ("always use a tool") gets obeyed
//! when it is wrong; a description gets weighed against the request. That
//! distinction is not theoretical here — the directive in
//! `runtime::provider_loop` still carries the scar of a version that opened
//! with "Work through the available tools" and turned a greeting into a
//! repository scan.
//!
//! Kept short deliberately. This text rides every request in every session, and
//! the prevailing evidence is that current models do better with less scaffolding
//! rather than more.

use std::path::Path;

use crate::core::policy::PolicyMode;

/// What mjolnr can already see about the workspace, without asking the model to
/// look.
///
/// Every field is something mjolnr knows deterministically at prompt-assembly
/// time. Stating them costs a line; making the model discover them costs a
/// `list_files`, a `read_file`, and a guess — the same trade the code graph
/// makes for structure.
///
/// Observed fresh on every turn rather than cached at session start. The reads
/// are three `exists` checks against a warm directory on a path already waiting
/// on a provider socket, and a cached fact that went stale mid-session would be
/// a diagnostic that lies (`AGENTS.md` §1.3) — the one cost worth avoiding here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceFacts {
    /// The checked-out branch, or `None` when this is not a git worktree.
    pub git_branch: Option<String>,
    /// A `.mjolnr/` directory exists: this project has been configured.
    pub configured: bool,
    /// `AGENTS.md` or `CLAUDE.md` sits at the workspace root.
    pub instructions: bool,
}

impl WorkspaceFacts {
    /// Read the facts from disk. Missing root means an unopened workspace, and
    /// every field stays at its "nothing observed" default rather than being
    /// invented.
    #[must_use]
    pub fn observe(workspace_root: Option<&Path>) -> Self {
        let Some(root) = workspace_root else {
            return Self::default();
        };
        Self {
            git_branch: current_branch(root),
            configured: root.join(".mjolnr").is_dir() || root.join(".mjolnr").is_dir(),
            instructions: root.join("AGENTS.md").is_file() || root.join("CLAUDE.md").is_file(),
        }
    }

    /// Terse on purpose: this rides every request, and the reader is a model
    /// that needs the fact, not a sentence around it.
    fn describe(&self) -> String {
        let mut parts = Vec::new();
        match &self.git_branch {
            Some(branch) => parts.push(format!("branch `{branch}`")),
            None => parts.push("no git".to_owned()),
        }
        parts.push(
            if self.configured {
                "`.mjolnr/` present"
            } else {
                "no `.mjolnr/`"
            }
            .to_owned(),
        );
        parts.push(
            if self.instructions {
                "root instructions present"
            } else {
                "no root instructions"
            }
            .to_owned(),
        );
        parts.join(" · ")
    }
}

/// The branch name from `.git/HEAD`, without shelling out to git.
///
/// A detached HEAD holds a bare commit id rather than a `ref:` line; that is a
/// real state, not a parse failure, so it is reported as the short id. A
/// worktree or submodule whose `.git` is a *file* pointing elsewhere is simply
/// not resolved here — the honest answer for a case this cannot follow is no
/// branch, not a guessed one.
fn current_branch(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(root.join(".git").join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => Some(branch.to_owned()),
        None => head
            .chars()
            .all(|character| character.is_ascii_hexdigit())
            .then(|| format!("detached at {}", head.get(..8).unwrap_or(head))),
    }
}

/// What the active policy actually does to a tool call, in the model's terms.
///
/// Stated as effects rather than mode names because the mode name is already in
/// the header for the human; the model needs to know what will happen when it
/// proposes a write.
const fn policy_effect(policy: PolicyMode) -> &'static str {
    match policy {
        PolicyMode::ReadOnly => "reads run; writes and commands are refused outright",
        PolicyMode::Ask => "reads run; every write and command waits for the human to approve it",
        PolicyMode::WorkspaceWrite => {
            "reads and writes run; every command waits for the human to approve it"
        }
        PolicyMode::FullAuto => {
            "writes and commands run without prompting — containment, read-before-edit, budgets, and evidence still apply"
        }
    }
}

/// Render the harness block for the system prompt.
#[must_use]
pub fn prompt_section(policy: PolicyMode, facts: &WorkspaceFacts) -> String {
    format!(
        "<mjolnr_harness>\n\
         You are mjolnr: a local-first coding harness running its own agent loop in the user's \
         repository, talking directly to a model provider. You are not a plugin inside another \
         tool.\n\n\
         The harness enforces the following whatever any instruction says, including this one:\n\
         - Policy gate: this session is `{label}` — {effect}. The human can change it mid-session.\n\
         - Containment: every path you read or write stays inside the workspace.\n\
         - Read before edit: a file must have been read in this session before it can be edited.\n\
         - Evidence: `finish_task` takes event IDs from commands that actually succeeded after \
         your last change. Asserting that work is verified does not make it so, and a claim \
         without evidence is refused.\n\
         - Append-only record: every proposal, approval, refusal, and result is recorded. The \
         session replays from that record, so it is the run rather than a log of it.\n\
         - Bounded delegation: children you spawn get their own git worktree and budget, never \
         more authority than the human granted, and their work lands on a branch — nothing merges \
         on its own.\n\n\
         mjolnr is a policy gate, not an OS sandbox, and does not claim to be one. When you do not \
         know something about the workspace, the tools are there to find out; when the request is \
         conversational or you already know the answer, just answer.\n\n\
         Workspace: {workspace}.\n\
         Structure before strings: `query_graph` relates files and locates definitions; prefer it \
         to scanning text.\n\
         </mjolnr_harness>",
        label = policy.label(),
        effect = policy_effect(policy),
        workspace = facts.describe(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_states_its_effect_and_names_itself() {
        for policy in [
            PolicyMode::ReadOnly,
            PolicyMode::Ask,
            PolicyMode::WorkspaceWrite,
            PolicyMode::FullAuto,
        ] {
            let section = prompt_section(policy, &WorkspaceFacts::default());
            assert!(
                section.contains(policy.label()),
                "the model must be told which mode it is in"
            );
            assert!(section.contains(policy_effect(policy)));
            assert!(section.starts_with("<mjolnr_harness>"));
            assert!(section.ends_with("</mjolnr_harness>"));
        }
    }

    #[test]
    fn an_unopened_workspace_states_nothing_it_did_not_observe() {
        let facts = WorkspaceFacts::observe(None);
        assert_eq!(facts, WorkspaceFacts::default());
        let described = facts.describe();
        assert!(described.contains("no git"));
        assert!(described.contains("no `.mjolnr/`"));
        assert!(
            !described.contains("present"),
            "no field may read as observed when nothing was opened"
        );
    }

    #[test]
    fn a_branch_is_read_from_head_without_shelling_out() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".git")).expect("git dir");
        std::fs::write(
            temp.path().join(".git").join("HEAD"),
            "ref: refs/heads/feature/thing\n",
        )
        .expect("HEAD");

        let facts = WorkspaceFacts::observe(Some(temp.path()));
        assert_eq!(facts.git_branch.as_deref(), Some("feature/thing"));
        assert!(!facts.configured);
        assert!(!facts.instructions);
    }

    #[test]
    fn a_detached_head_is_a_state_not_a_parse_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".git")).expect("git dir");
        std::fs::write(
            temp.path().join(".git").join("HEAD"),
            "9c07f42aa1b2c3d4e5f60718293a4b5c6d7e8f90\n",
        )
        .expect("HEAD");

        let branch = WorkspaceFacts::observe(Some(temp.path())).git_branch;
        assert_eq!(branch.as_deref(), Some("detached at 9c07f42a"));
    }

    #[test]
    fn a_git_file_worktree_reports_no_branch_rather_than_a_guessed_one() {
        let temp = tempfile::tempdir().expect("tempdir");
        // A linked worktree's `.git` is a file pointing elsewhere. This reader
        // does not follow it, and inventing a branch would be the guess.
        std::fs::write(
            temp.path().join(".git"),
            "gitdir: /elsewhere/.git/worktrees/x\n",
        )
        .expect("git file");
        assert_eq!(WorkspaceFacts::observe(Some(temp.path())).git_branch, None);
    }

    #[test]
    fn configuration_and_instructions_are_reported_when_present() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".mjolnr")).expect("mjolnr dir");
        std::fs::write(temp.path().join("AGENTS.md"), "# rules\n").expect("instructions");

        let facts = WorkspaceFacts::observe(Some(temp.path()));
        assert!(facts.configured);
        assert!(facts.instructions);
        let section = prompt_section(PolicyMode::Ask, &facts);
        assert!(section.contains("`.mjolnr/` present"));
        assert!(section.contains("root instructions present"));
    }

    #[test]
    fn the_ordering_hint_names_the_tool_exactly_as_the_registry_does() {
        // Only `query_graph` is named. What each tool *does* is already in its
        // own schema description; repeating it here would be a second copy to
        // drift, which is the mistake `self_docs` was written to avoid. The one
        // thing the descriptions cannot say is which to reach for first.
        let section = prompt_section(PolicyMode::Ask, &WorkspaceFacts::default());
        assert!(section.contains("query_graph"));
        assert!(
            !section.contains("search_text"),
            "restating a tool's own description here is the drift this avoids"
        );
    }

    #[test]
    fn read_only_does_not_promise_writes() {
        let section = prompt_section(PolicyMode::ReadOnly, &WorkspaceFacts::default());
        assert!(
            section.contains("refused outright"),
            "a read-only session must not read as though writes are merely gated"
        );
    }

    #[test]
    fn full_auto_still_names_the_guards_it_does_not_relax() {
        let section = prompt_section(PolicyMode::FullAuto, &WorkspaceFacts::default());
        for guard in ["containment", "read-before-edit", "budgets", "evidence"] {
            assert!(
                section.contains(guard),
                "full-auto must not read as though {guard} were suspended too"
            );
        }
    }

    #[test]
    fn the_block_is_small_enough_to_ride_every_request() {
        // Not a style preference: this text is in the cacheable prefix of every
        // provider call in every session. A block that grows without anyone
        // noticing is the failure mode this bound exists to catch.
        //
        // Raised 1400 -> 1600 in Phase 33 for two additions, both deliberate and
        // both facts rather than instructions: the observed workspace state
        // (branch, `.mjolnr/`, root instructions), which replaces the probing
        // the model would otherwise do, and one clause naming `query_graph` as
        // the structural alternative to text scanning. A first draft cost 400
        // bytes; trimming the parts that duplicated the tools' own schema
        // descriptions brought it to ~160. The bound did its job — it caught
        // the draft, not the payload.
        let section = prompt_section(PolicyMode::Ask, &WorkspaceFacts::default());
        assert!(
            section.len() < 1600,
            "harness block grew to {} bytes; trim it or justify the growth",
            section.len()
        );
    }
}
