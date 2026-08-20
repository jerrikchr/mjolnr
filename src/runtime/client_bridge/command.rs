//! Translation from `ClientCommand` to runtime `MjolnrCommand`.

use std::path::PathBuf;

use crate::core::client::{
    ClientCommand, MAX_BRANCH_NAME_BYTES, MAX_CAPTURE_DIGEST_BYTES, MAX_CLONE_DESTINATION_BYTES,
    MAX_CLONE_SOURCE_BYTES, MAX_COMMIT_MESSAGE_BYTES, MAX_COUNCIL_NOTE_BYTES,
    MAX_INTEGRATION_ID_BYTES, MAX_REBASE_TARGET_BYTES, MAX_REMOTE_TASK_ID_BYTES,
    MAX_REPOSITORY_PATH_BYTES, MAX_REPOSITORY_PATHS, MAX_REVIEW_NOTE_BYTES,
    MAX_REVIEW_THREADS_PER_REQUEST, MAX_SAVE_TEXT_BYTES,
};
use crate::core::command::{ApprovalId, MjolnrCommand};
use crate::core::directive::DirectiveSource;
use crate::core::error::ReasonCode;
use crate::core::event::SessionId;
use crate::core::review::ReviewThreadId;
use crate::integrations::{MAX_REMOTE_BODY_BYTES, MAX_REMOTE_TITLE_BYTES};

use super::bridge::ClientBridgeError;

fn map_save_file(
    path: &str,
    expected_digest: &str,
    text: &str,
) -> Result<MjolnrCommand, ClientBridgeError> {
    validate_workspace_file_path(path)?;
    validate_capture_digest(expected_digest)?;
    validate_save_text(text)?;
    Ok(MjolnrCommand::SaveFile {
        path: path.to_owned(),
        expected_digest: expected_digest.to_owned(),
        text: text.to_owned(),
    })
}

fn map_session_control_command(
    command: &ClientCommand,
) -> Result<Option<MjolnrCommand>, ClientBridgeError> {
    let mapped = match command {
        ClientCommand::CreateSession { provider, model } => {
            if provider.trim().is_empty() || model.trim().is_empty() {
                return Err(invalid(
                    "model",
                    "a provider and model are required to create a session",
                ));
            }
            MjolnrCommand::CreateSession {
                provider: crate::core::model::ProviderId::new(provider),
                model: crate::core::model::ModelId::new(model),
            }
        }
        ClientCommand::ResumeSession { session } => MjolnrCommand::ResumeSession {
            session: parse_session(session)?,
        },
        ClientCommand::ResolveResume { choice } => MjolnrCommand::ResolveResume {
            choice: (*choice).into(),
        },
        ClientCommand::SendMessage { text } => {
            if text.trim().is_empty() {
                return Err(invalid("text", "an empty message is not a directive"));
            }
            MjolnrCommand::SendUserMessage {
                text: text.clone(),
                source: DirectiveSource::Human,
            }
        }
        ClientCommand::CancelRun => MjolnrCommand::CancelRun,
        ClientCommand::ResolveApproval { approval, decision } => MjolnrCommand::ResolveApproval {
            approval: parse_approval(approval)?,
            decision: (*decision).into(),
        },
        ClientCommand::ResolveRecovery { decision } => MjolnrCommand::ResolveRecovery {
            decision: (*decision).into(),
        },
        ClientCommand::SetPolicy { policy } => MjolnrCommand::SetPolicy {
            mode: (*policy).into(),
        },
        ClientCommand::EndSession => MjolnrCommand::EndSession,
        _ => return Ok(None),
    };
    Ok(Some(mapped))
}

pub(super) fn command_to_mjolnr(
    command: &ClientCommand,
) -> Result<Option<MjolnrCommand>, ClientBridgeError> {
    if let Some(session_cmd) = map_session_control_command(command)? {
        return Ok(Some(session_cmd));
    }
    let mapped = match command {
        ClientCommand::OpenProject { root } => map_open_project(root)?,
        ClientCommand::RefreshRepository => MjolnrCommand::RefreshRepository,
        ClientCommand::SaveFile {
            path,
            expected_digest,
            text,
        } => map_save_file(path, expected_digest, text)?,
        ClientCommand::StartPlanInterview { goal } => map_start_plan_interview(goal)?,
        ClientCommand::AskPlanQuestion { .. }
        | ClientCommand::AnswerPlanQuestion { .. }
        | ClientCommand::ProposePlan { .. }
        | ClientCommand::ReviewPlan { .. }
        | ClientCommand::ApprovePlan { .. }
        | ClientCommand::HandoffPlan { .. } => return map_plan_command(command).map(Some),
        ClientCommand::CreateWorktree { .. }
        | ClientCommand::ForkWork { .. }
        | ClientCommand::StartChild { .. }
        | ClientCommand::CancelChild { .. }
        | ClientCommand::PreserveBranch { .. }
        | ClientCommand::SettleChild { .. }
        | ClientCommand::DiscardSettledWorktree { .. } => map_child_run_command(command)?,
        ClientCommand::StagePaths { .. }
        | ClientCommand::CloneProject { .. }
        | ClientCommand::StageHunks { .. }
        | ClientCommand::Unstage { .. }
        | ClientCommand::CreateBranch { .. }
        | ClientCommand::Commit { .. }
        | ClientCommand::IntegrateChildBranch { .. }
        | ClientCommand::Fetch
        | ClientCommand::Push { .. }
        | ClientCommand::IntegrateUpstream { .. }
        | ClientCommand::Rebase { .. }
        | ClientCommand::AbortRebase => map_repository_command(command)?,
        ClientCommand::FetchTask { .. }
        | ClientCommand::FetchTasks { .. }
        | ClientCommand::SubmitChange { .. } => map_integration_command(command)?,
        ClientCommand::AddReviewNote { .. }
        | ClientCommand::AddReviewComment { .. }
        | ClientCommand::SendReviewNotes { .. }
        | ClientCommand::ResolveCouncilFinding { .. }
        | ClientCommand::ProposeCouncilAmendment { .. } => map_review_command(command)?,
        ClientCommand::OpenDecisionTicket { .. }
        | ClientCommand::ResolveDecisionTicket { .. }
        | ClientCommand::ImportWorkItem { .. }
        | ClientCommand::RefreshImportedItem { .. }
        | ClientCommand::SubmitImportedComment { .. } => map_board_command(command)?,
        ClientCommand::RollbackToCheckpoint {
            target_sequence,
            expected_head,
        } => MjolnrCommand::RollbackToCheckpoint {
            target_sequence: *target_sequence,
            expected_head: expected_head.clone(),
        },
        ClientCommand::ExternalAgentList
        | ClientCommand::ExternalAgentLaunch { .. }
        | ClientCommand::ExternalAgentStop { .. }
        | ClientCommand::ExternalAgentImport { .. } => {
            return map_external_agent_command(command).map(Some);
        }
        ClientCommand::RefreshCredentials => MjolnrCommand::RefreshCredentials,
        _ => return Ok(None),
    };
    Ok(Some(mapped))
}

/// Map and validate the Phase D3 review family.
///
/// The note body is human text, so it is bounded rather than
/// character-restricted — but it is bounded *here*, at the bridge, because it
/// reaches two places that both need a ceiling: the durable record, and the
/// directive `sendReviewNotes` assembles from it. The thread-count bound on a
/// send exists for the second of those; without it the per-note ceiling bounds
/// nothing that matters.
fn map_review_command(command: &ClientCommand) -> Result<MjolnrCommand, ClientBridgeError> {
    let mapped = match command {
        ClientCommand::AddReviewNote {
            path,
            side,
            line,
            capture_digest,
            body,
        } => {
            validate_repository_path(path)?;
            validate_capture_digest(capture_digest)?;
            validate_review_body(body)?;
            if *line == 0 {
                // Diff line numbers are 1-based, so zero names no line at all.
                // Refused here rather than reaching the anchor resolver as a
                // lookup that would simply fail to match.
                return Err(invalid("line", "a diff line number starts at 1"));
            }
            MjolnrCommand::AddReviewNote {
                path: path.clone(),
                side: (*side).into(),
                line: *line,
                capture_digest: capture_digest.clone(),
                body: body.clone(),
            }
        }
        ClientCommand::AddReviewComment { thread_id, body } => {
            validate_review_body(body)?;
            MjolnrCommand::AddReviewComment {
                thread: parse_review_thread(thread_id)?,
                body: body.clone(),
            }
        }
        ClientCommand::SendReviewNotes { thread_ids } => {
            if thread_ids.is_empty() {
                return Err(invalid(
                    "threadIds",
                    "at least one review thread is required: an empty request asks mjolnr for \
                     nothing",
                ));
            }
            if thread_ids.len() > MAX_REVIEW_THREADS_PER_REQUEST {
                return Err(invalid(
                    "threadIds",
                    &format!(
                        "at most {MAX_REVIEW_THREADS_PER_REQUEST} review threads may be sent in \
                         one request"
                    ),
                ));
            }
            let threads = thread_ids
                .iter()
                .map(|id| parse_review_thread(id))
                .collect::<Result<Vec<_>, _>>()?;
            MjolnrCommand::SendReviewNotes { threads }
        }
        ClientCommand::ResolveCouncilFinding {
            review_id,
            finding_id,
            disposition,
            note,
        } => {
            validate_council_note(note.as_deref())?;
            MjolnrCommand::ResolveCouncilFinding {
                review_id: parse_council_review_id(review_id)?,
                finding_id: parse_council_finding_id(finding_id)?,
                disposition: match disposition {
                    crate::core::client::ClientCouncilDisposition::Accept => {
                        crate::core::council::CouncilDisposition::Accept
                    }
                    crate::core::client::ClientCouncilDisposition::Reject => {
                        crate::core::council::CouncilDisposition::Reject
                    }
                    crate::core::client::ClientCouncilDisposition::Defer => {
                        crate::core::council::CouncilDisposition::Defer
                    }
                },
                note: note.clone(),
            }
        }
        ClientCommand::ProposeCouncilAmendment { review_id } => {
            MjolnrCommand::ProposeCouncilAmendment {
                review_id: parse_council_review_id(review_id)?,
            }
        }
        // The caller's match guarantees a review variant; a bug there must
        // surface as a typed refusal, not a panic.
        _ => {
            return Err(invalid(
                "command",
                "internal routing error: not a review command",
            ));
        }
    };
    Ok(mapped)
}

/// Map and validate the Phase E5 decision-ticket family plus the step-4b
/// imported-item family.
///
/// The bridge owns the input-shape rules: bounded text, at least two options,
/// parsed and de-duplicated blockers for tickets; bounded external text for
/// imports and sync-back comments (same shape as `RemoteTask`'s constructor).
/// What the bridge *cannot* know — whether the named tickets exist, and
/// whether the chosen option is recorded on the ticket — is the runtime's,
/// because both questions need state.
fn map_board_command(command: &ClientCommand) -> Result<MjolnrCommand, ClientBridgeError> {
    let mapped = match command {
        ClientCommand::SubmitImportedComment {
            integration,
            remote_id,
            expected_revision,
            body,
        } => {
            validate_integration_id(integration)?;
            validate_remote_task_id(remote_id)?;
            validate_revision_pin_named("expectedRevision", expected_revision)?;
            validate_remote_text("body", body, MAX_REMOTE_BODY_BYTES)?;
            MjolnrCommand::SubmitImportedComment {
                integration: integration.clone(),
                remote_id: remote_id.clone(),
                expected_revision: expected_revision.clone(),
                body: body.clone(),
            }
        }
        ClientCommand::OpenDecisionTicket {
            question,
            kind,
            options,
            blocked_by,
        } => {
            validate_ticket_question(question)?;
            validate_ticket_options(options)?;
            let blockers = parse_ticket_blockers(blocked_by)?;
            MjolnrCommand::OpenDecisionTicket {
                question: question.clone(),
                kind: (*kind).into(),
                options: options.clone(),
                blocked_by: blockers,
            }
        }
        ClientCommand::ResolveDecisionTicket {
            ticket,
            chosen_option,
            note,
        } => {
            validate_ticket_note(note.as_deref())?;
            MjolnrCommand::ResolveDecisionTicket {
                ticket: parse_ticket_id(ticket)?,
                chosen_option: usize::try_from(*chosen_option).map_err(|_| {
                    invalid("chosenOption", "the option index does not fit this build")
                })?,
                note: note.clone(),
            }
        }
        ClientCommand::ImportWorkItem { item } => {
            validate_imported_item(item)?;
            MjolnrCommand::ImportWorkItem { item: item.clone() }
        }
        ClientCommand::RefreshImportedItem {
            expected_revision,
            item,
        } => {
            validate_imported_item(item)?;
            validate_revision_pin(expected_revision)?;
            MjolnrCommand::RefreshImportedItem {
                expected_revision: expected_revision.clone(),
                item: item.clone(),
            }
        }
        // The caller's match guarantees a board variant; a bug there must
        // surface as a typed refusal, not a panic.
        _ => {
            return Err(invalid(
                "command",
                "internal routing error: not a board command",
            ));
        }
    };
    Ok(mapped)
}

/// Validate one imported item at the bridge, before it can reach the durable
/// record.
///
/// The imported title is the one field here a third party authored, so it gets
/// the same treatment `validate_remote_text` gives an issue title: bounded, and
/// refused outright when it carries a control character — an ANSI escape in a
/// GitHub issue title reaching a terminal client is the exact path that guard
/// exists to close (AGENTS.md §5's bounded-output rule). The identifiers
/// (`integration`, `remote_id`, `fetched_revision`) are not prose, so they are
/// refused any control character at all, same as a remote task id.
fn validate_imported_item(
    item: &crate::core::imported::ImportedItem,
) -> Result<(), ClientBridgeError> {
    use crate::integrations::{MAX_REMOTE_SOURCE_URL_BYTES, MAX_REMOTE_TITLE_BYTES};
    if item.integration.trim().is_empty() {
        return Err(invalid("integration", "an integration label is required"));
    }
    if item.integration.len() > 64 {
        return Err(invalid(
            "integration",
            "an integration label may not exceed 64 bytes",
        ));
    }
    validate_identifier("integration", &item.integration)?;
    if item.remote_id.trim().is_empty() {
        return Err(invalid("remoteId", "a remote id is required"));
    }
    if item.remote_id.len() > 256 {
        return Err(invalid("remoteId", "a remote id may not exceed 256 bytes"));
    }
    validate_identifier("remoteId", &item.remote_id)?;
    if item.fetched_revision.trim().is_empty() {
        return Err(invalid(
            "fetchedRevision",
            "a fetched revision is required: it is the revision the item was rendered for",
        ));
    }
    if item.fetched_revision.len() > 512 {
        return Err(invalid(
            "fetchedRevision",
            "a fetched revision may not exceed 512 bytes",
        ));
    }
    validate_identifier("fetchedRevision", &item.fetched_revision)?;
    if item.source_url.len() > MAX_REMOTE_SOURCE_URL_BYTES {
        return Err(invalid(
            "sourceUrl",
            &format!("a source url may not exceed {MAX_REMOTE_SOURCE_URL_BYTES} bytes"),
        ));
    }
    validate_identifier("sourceUrl", &item.source_url)?;
    if item.title.trim().is_empty() {
        return Err(invalid("title", "an imported item title is required"));
    }
    validate_remote_text("title", &item.title, MAX_REMOTE_TITLE_BYTES)?;
    if item.blocked_by.len() > crate::core::client::MAX_TICKET_BLOCKERS {
        return Err(invalid(
            "blockedBy",
            &format!(
                "at most {} blockers may be recorded on an imported item",
                crate::core::client::MAX_TICKET_BLOCKERS
            ),
        ));
    }
    let _ = serde_json::to_value(item).map_err(|error| {
        invalid(
            "item",
            &format!("an imported item could not be serialized: {error}"),
        )
    })?;
    Ok(())
}

/// An identifier, not prose: any control character at all is refused, because
/// an identifier has no legitimate business carrying one (a remote task id's
/// rule, applied to every field that names rather than describes).
fn validate_identifier(field: &'static str, value: &str) -> Result<(), ClientBridgeError> {
    if value.chars().any(char::is_control) {
        return Err(invalid(
            field,
            "this identifier may not contain control characters",
        ));
    }
    Ok(())
}

/// The revision pin a refresh or a change is approved against. Required and
/// bounded, like the revision it is compared with; an identifier, so no control
/// characters. The bound is the integration boundary's own
/// [`MAX_REMOTE_REVISION_BYTES`](crate::integrations::MAX_REMOTE_REVISION_BYTES),
/// so the two places that check a pin cannot disagree about its size.
fn validate_revision_pin(expected_revision: &str) -> Result<(), ClientBridgeError> {
    validate_revision_pin_named("expectedRevision", expected_revision)
}

fn validate_revision_pin_named(
    field: &'static str,
    revision: &str,
) -> Result<(), ClientBridgeError> {
    use crate::integrations::MAX_REMOTE_REVISION_BYTES;
    if revision.trim().is_empty() {
        return Err(invalid(field, "a revision is required"));
    }
    if revision.len() > MAX_REMOTE_REVISION_BYTES {
        return Err(invalid(
            field,
            &format!("a revision may not exceed {MAX_REMOTE_REVISION_BYTES} bytes"),
        ));
    }
    validate_identifier(field, revision)
}

/// Map and validate the Phase D2 child-run family. Inputs are bounded here,
/// at the bridge, so `git worktree` and the agent loop never receive a
/// hostile identifier when execution lands.
fn map_child_run_command(command: &ClientCommand) -> Result<MjolnrCommand, ClientBridgeError> {
    let mapped = match command {
        ClientCommand::CreateWorktree {
            name,
            base_revision,
        } => {
            validate_child_run_name(name)?;
            validate_base_revision(base_revision)?;
            MjolnrCommand::CreateWorktree {
                name: name.clone(),
                base_revision: base_revision.clone(),
            }
        }
        ClientCommand::ForkWork {
            name,
            base_revision,
        } => {
            validate_child_run_name(name)?;
            validate_base_revision(base_revision)?;
            MjolnrCommand::ForkWork {
                name: name.clone(),
                base_revision: base_revision.clone(),
            }
        }
        ClientCommand::StartChild {
            name,
            directive,
            policy_ceiling,
            budget,
        } => {
            validate_child_run_name(name)?;
            validate_child_directive(directive)?;
            MjolnrCommand::StartChild {
                name: name.clone(),
                directive: directive.clone(),
                policy_ceiling: policy_ceiling.map(Into::into),
                budget: *budget,
            }
        }
        ClientCommand::CancelChild { name } => {
            validate_child_run_name(name)?;
            MjolnrCommand::CancelChild { name: name.clone() }
        }
        ClientCommand::PreserveBranch { name } => {
            validate_child_run_name(name)?;
            MjolnrCommand::PreserveBranch { name: name.clone() }
        }
        ClientCommand::SettleChild { name } => {
            validate_child_run_name(name)?;
            MjolnrCommand::SettleChild { name: name.clone() }
        }
        ClientCommand::DiscardSettledWorktree { name } => {
            validate_child_run_name(name)?;
            MjolnrCommand::DiscardSettledWorktree { name: name.clone() }
        }
        // The caller's match guarantees a child-run variant; a bug there must
        // surface as a typed refusal, not a panic.
        _ => {
            return Err(invalid(
                "command",
                "internal routing error: not a child-run command",
            ));
        }
    };
    Ok(mapped)
}

/// Map and validate the Phase D5 repository family. Every value here becomes
/// an argv element of a `git` invocation, so the bridge is where it stops
/// being arbitrary client text: bounded, flag-safe, and control-character
/// free before the runtime ever spawns a process.
fn map_repository_command(command: &ClientCommand) -> Result<MjolnrCommand, ClientBridgeError> {
    let mapped = match command {
        ClientCommand::StagePaths { paths } => {
            validate_repository_paths(paths)?;
            MjolnrCommand::StagePaths {
                paths: paths.clone(),
            }
        }
        ClientCommand::StageHunks { path, hunk_indices } => {
            validate_repository_path(path)?;
            if hunk_indices.is_empty() {
                return Err(invalid("hunkIndices", "at least one hunk is required"));
            }
            if hunk_indices.len() > MAX_REPOSITORY_PATHS {
                return Err(invalid(
                    "hunkIndices",
                    &format!("at most {MAX_REPOSITORY_PATHS} hunks may be staged in one command"),
                ));
            }
            MjolnrCommand::StageHunks {
                path: path.clone(),
                hunk_indices: hunk_indices.clone(),
            }
        }
        ClientCommand::Unstage { paths } => {
            validate_repository_paths(paths)?;
            MjolnrCommand::Unstage {
                paths: paths.clone(),
            }
        }
        ClientCommand::CreateBranch {
            name,
            base_revision,
        } => {
            validate_branch_name(name)?;
            validate_base_revision(base_revision)?;
            MjolnrCommand::CreateBranch {
                name: name.clone(),
                base_revision: base_revision.clone(),
            }
        }
        ClientCommand::Commit {
            message,
            expected_index_revision,
        } => {
            validate_commit_message(message)?;
            validate_base_revision(expected_index_revision)?;
            MjolnrCommand::Commit {
                message: message.clone(),
                expected_index_revision: expected_index_revision.clone(),
            }
        }
        ClientCommand::IntegrateChildBranch {
            name,
            message,
            expected_head,
        } => {
            validate_branch_name(name)?;
            validate_commit_message(message)?;
            validate_base_revision(expected_head)?;
            MjolnrCommand::IntegrateChildBranch {
                name: name.clone(),
                message: message.clone(),
                expected_head: expected_head.clone(),
            }
        }
        ClientCommand::Fetch => MjolnrCommand::Fetch,
        ClientCommand::Push { expected_head } => {
            validate_base_revision(expected_head)?;
            MjolnrCommand::Push {
                expected_head: expected_head.clone(),
            }
        }
        ClientCommand::IntegrateUpstream {
            message,
            expected_head,
        } => {
            // A merge commit's message is a human act, like a commit's; an
            // absent expected revision makes the staleness guard opt-in.
            validate_commit_message(message)?;
            validate_base_revision(expected_head)?;
            MjolnrCommand::IntegrateUpstream {
                message: message.clone(),
                expected_head: expected_head.clone(),
            }
        }
        ClientCommand::CloneProject {
            source,
            destination,
        } => map_clone_project(source, destination)?,
        ClientCommand::Rebase {
            onto,
            expected_head,
        } => map_rebase_command(onto, expected_head)?,
        ClientCommand::AbortRebase => MjolnrCommand::AbortRebase,
        // The caller's match guarantees a repository variant; a bug there must
        // surface as a typed refusal, not a panic.
        _ => {
            return Err(invalid(
                "command",
                "internal routing error: not a repository command",
            ));
        }
    };
    Ok(mapped)
}

fn map_clone_project(source: &str, destination: &str) -> Result<MjolnrCommand, ClientBridgeError> {
    validate_clone_source(source)?;
    let destination_path = PathBuf::from(destination);
    if !destination_path.is_absolute() {
        return Err(invalid(
            "destination",
            "a clone destination must be an absolute path",
        ));
    }
    if destination.len() > MAX_CLONE_DESTINATION_BYTES {
        return Err(invalid(
            "destination",
            &format!("a clone destination may not exceed {MAX_CLONE_DESTINATION_BYTES} bytes"),
        ));
    }
    Ok(MjolnrCommand::CloneProject {
        source: source.to_owned(),
        destination: destination_path,
    })
}

fn map_rebase_command(onto: &str, expected_head: &str) -> Result<MjolnrCommand, ClientBridgeError> {
    validate_rebase_target(onto)?;
    validate_base_revision(expected_head)?;
    Ok(MjolnrCommand::Rebase {
        onto: onto.to_owned(),
        expected_head: expected_head.to_owned(),
    })
}

/// Map and validate the Phase D6 integration family.
///
/// The title and body here are text a third party wrote in an issue or a pull
/// request. They are bounded and stripped of control characters at the bridge
/// so they arrive in the durable record as data, not as something that could
/// reformat a terminal or a log line. They are never treated as authority —
/// that contract lives in `integrations::RemoteTask::framed_for_model`.
fn map_integration_command(command: &ClientCommand) -> Result<MjolnrCommand, ClientBridgeError> {
    let mapped = match command {
        ClientCommand::FetchTask { source, task_id } => {
            validate_integration_id(source)?;
            validate_remote_task_id(task_id)?;
            MjolnrCommand::FetchTask {
                source: source.clone(),
                task_id: task_id.clone(),
            }
        }
        ClientCommand::FetchTasks { source, task_ids } => {
            validate_integration_id(source)?;
            if task_ids.is_empty() {
                return Err(invalid(
                    "taskIds",
                    "at least one remote task id is required",
                ));
            }
            if task_ids.len() > crate::core::client::MAX_FETCH_BATCH_SIZE {
                return Err(invalid(
                    "taskIds",
                    &format!(
                        "at most {} remote task ids may be fetched at once",
                        crate::core::client::MAX_FETCH_BATCH_SIZE
                    ),
                ));
            }
            for task_id in task_ids {
                validate_remote_task_id(task_id)?;
            }
            MjolnrCommand::FetchTasks {
                source: source.clone(),
                task_ids: task_ids.clone(),
            }
        }
        ClientCommand::SubmitChange { source, request } => {
            validate_integration_id(source)?;
            validate_remote_task_id(&request.remote_id)?;
            validate_revision_pin(&request.expected_revision)?;
            validate_revision_pin_named("headCommit", &request.head_commit)?;
            validate_branch_name(&request.head_branch)?;
            validate_branch_name(&request.base_branch)?;
            validate_remote_text("title", &request.title, MAX_REMOTE_TITLE_BYTES)?;
            validate_remote_text("body", &request.body, MAX_REMOTE_BODY_BYTES)?;
            MjolnrCommand::SubmitChange {
                source: source.clone(),
                remote_id: request.remote_id.clone(),
                expected_revision: request.expected_revision.clone(),
                title: request.title.clone(),
                body: request.body.clone(),
                head_commit: request.head_commit.clone(),
                head_branch: request.head_branch.clone(),
                base_branch: request.base_branch.clone(),
            }
        }
        ClientCommand::SubmitImportedComment {
            integration,
            remote_id,
            expected_revision,
            body,
        } => {
            validate_integration_id(integration)?;
            validate_remote_task_id(remote_id)?;
            validate_revision_pin_named("expectedRevision", expected_revision)?;
            validate_remote_text("body", body, MAX_REMOTE_BODY_BYTES)?;
            MjolnrCommand::SubmitImportedComment {
                integration: integration.clone(),
                remote_id: remote_id.clone(),
                expected_revision: expected_revision.clone(),
                body: body.clone(),
            }
        }
        // The caller's match guarantees an integration variant; a bug there
        // must surface as a typed refusal, not a panic.
        _ => {
            return Err(invalid(
                "command",
                "internal routing error: not an integration command",
            ));
        }
    };
    Ok(mapped)
}

fn map_plan_command(command: &ClientCommand) -> Result<MjolnrCommand, ClientBridgeError> {
    if let ClientCommand::StartPlanInterview { goal } = command {
        return map_start_plan_interview(goal);
    }
    let mapped = match command {
        ClientCommand::AskPlanQuestion {
            plan_id,
            prompt,
            options,
            is_multi_select,
        } => MjolnrCommand::AskPlanQuestion {
            plan_id: parse_plan_id(plan_id)?,
            question: crate::core::plan::Question {
                id: crate::core::plan::QuestionId::new(),
                prompt: prompt.clone(),
                options: options.clone(),
                is_multi_select: *is_multi_select,
                created_at: time::OffsetDateTime::now_utc(),
            },
        },
        ClientCommand::AnswerPlanQuestion {
            plan_id,
            question_id,
            selected_options,
            freeform_text,
        } => MjolnrCommand::AnswerPlanQuestion {
            plan_id: parse_plan_id(plan_id)?,
            answer: crate::core::plan::QuestionAnswer {
                question_id: parse_question_id(question_id)?,
                selected_options: selected_options.clone(),
                freeform_text: freeform_text.clone(),
                answered_at: time::OffsetDateTime::now_utc(),
            },
        },
        ClientCommand::ProposePlan {
            plan_id,
            revision,
            title,
            summary,
            steps,
        } => MjolnrCommand::ProposePlan {
            proposal: crate::core::plan::PlanProposal {
                plan_id: parse_plan_id(plan_id)?,
                revision_id: crate::core::plan::RevisionId::new(*revision),
                title: title.clone(),
                summary: summary.clone(),
                steps: map_plan_steps(steps),
                proposed_at: time::OffsetDateTime::now_utc(),
            },
        },
        ClientCommand::ReviewPlan {
            plan_id,
            revision,
            reviewer,
            verdict,
            feedback,
        } => MjolnrCommand::ReviewPlan {
            review: crate::core::plan::PlanReview {
                plan_id: parse_plan_id(plan_id)?,
                revision_id: crate::core::plan::RevisionId::new(*revision),
                reviewer: reviewer.clone(),
                verdict: (*verdict).into(),
                feedback: feedback.clone(),
                reviewed_at: time::OffsetDateTime::now_utc(),
            },
        },
        ClientCommand::ApprovePlan {
            plan_id,
            revision,
            decision,
            note,
        } => MjolnrCommand::ApprovePlan {
            approval: crate::core::plan::PlanApproval {
                plan_id: parse_plan_id(plan_id)?,
                revision_id: crate::core::plan::RevisionId::new(*revision),
                approver: "Human".to_string(),
                decision: (*decision).into(),
                note: note.clone(),
                approved_at: time::OffsetDateTime::now_utc(),
            },
        },
        ClientCommand::HandoffPlan {
            plan_id,
            revision,
            note,
        } => MjolnrCommand::HandoffPlan {
            handoff: crate::core::plan::PlanHandoff {
                plan_id: parse_plan_id(plan_id)?,
                revision_id: crate::core::plan::RevisionId::new(*revision),
                handoff_note: note.clone(),
                created_at: time::OffsetDateTime::now_utc(),
            },
        },
        _ => {
            return Err(invalid(
                "type",
                "only plan workflow commands use the plan mapper",
            ));
        }
    };
    Ok(mapped)
}

fn map_start_plan_interview(goal: &str) -> Result<MjolnrCommand, ClientBridgeError> {
    let goal = goal.trim();
    if goal.is_empty() {
        return Err(invalid("goal", "an interview goal is required"));
    }
    if goal.chars().count() > crate::core::plan::MAX_INTERVIEW_GOAL_CHARS {
        return Err(invalid(
            "goal",
            "an interview goal exceeds the 8000 character limit",
        ));
    }
    Ok(MjolnrCommand::StartPlanInterview {
        goal: goal.to_owned(),
    })
}

fn map_open_project(root: &str) -> Result<MjolnrCommand, ClientBridgeError> {
    if root.trim().is_empty() {
        return Err(invalid("root", "a project path is required"));
    }
    Ok(MjolnrCommand::OpenProject {
        root: PathBuf::from(root),
    })
}

fn map_plan_steps(
    steps: &[crate::core::client::ClientPlanStep],
) -> Vec<crate::core::plan::PlanStep> {
    steps
        .iter()
        .map(|step| crate::core::plan::PlanStep {
            index: step.index,
            title: step.title.clone(),
            description: step.description.clone(),
        })
        .collect()
}

fn parse_plan_id(raw: &str) -> Result<crate::core::plan::PlanId, ClientBridgeError> {
    uuid::Uuid::parse_str(raw.trim())
        .map(crate::core::plan::PlanId::from_uuid)
        .map_err(|_| invalid("plan_id", "a plan_id is its UUID text form"))
}

fn parse_question_id(raw: &str) -> Result<crate::core::plan::QuestionId, ClientBridgeError> {
    uuid::Uuid::parse_str(raw.trim())
        .map(crate::core::plan::QuestionId::from_uuid)
        .map_err(|_| invalid("question_id", "a question_id is its UUID text form"))
}

fn parse_council_review_id(
    raw: &str,
) -> Result<crate::core::council::CouncilReviewId, ClientBridgeError> {
    uuid::Uuid::parse_str(raw.trim())
        .map(crate::core::council::CouncilReviewId::from_uuid)
        .map_err(|_| invalid("review_id", "a review_id is its UUID text form"))
}

fn parse_council_finding_id(
    raw: &str,
) -> Result<crate::core::council::CouncilFindingId, ClientBridgeError> {
    uuid::Uuid::parse_str(raw.trim())
        .map(crate::core::council::CouncilFindingId::from_uuid)
        .map_err(|_| invalid("finding_id", "a finding_id is its UUID text form"))
}

fn validate_council_note(note: Option<&str>) -> Result<(), ClientBridgeError> {
    if let Some(note) = note
        && note.len() > MAX_COUNCIL_NOTE_BYTES
    {
        return Err(invalid(
            "note",
            "a council disposition note may not exceed 2048 bytes",
        ));
    }
    Ok(())
}

fn parse_ticket_id(raw: &str) -> Result<crate::core::board::DecisionTicketId, ClientBridgeError> {
    uuid::Uuid::parse_str(raw.trim())
        .map(crate::core::board::DecisionTicketId::from_uuid)
        .map_err(|_| invalid("ticket", "a ticket id is its UUID text form"))
}

fn parse_ticket_blockers(
    raw: &[String],
) -> Result<Vec<crate::core::board::DecisionTicketId>, ClientBridgeError> {
    if raw.len() > crate::core::client::MAX_TICKET_BLOCKERS {
        return Err(invalid(
            "blockedBy",
            &format!(
                "at most {} tickets may block one ticket",
                crate::core::client::MAX_TICKET_BLOCKERS
            ),
        ));
    }
    let mut ids = Vec::with_capacity(raw.len());
    for id in raw {
        let parsed = parse_ticket_id(id)?;
        if ids.contains(&parsed) {
            return Err(invalid(
                "blockedBy",
                "a duplicate blocker is meaningless, and a quiet dedupe would hide it",
            ));
        }
        ids.push(parsed);
    }
    Ok(ids)
}

fn validate_ticket_question(question: &str) -> Result<(), ClientBridgeError> {
    if question.trim().is_empty() {
        return Err(invalid("question", "a ticket's question is required"));
    }
    if question.len() > crate::core::client::MAX_TICKET_QUESTION_BYTES {
        return Err(invalid(
            "question",
            &format!(
                "a ticket's question may not exceed {} bytes",
                crate::core::client::MAX_TICKET_QUESTION_BYTES
            ),
        ));
    }
    Ok(())
}

fn validate_ticket_options(options: &[String]) -> Result<(), ClientBridgeError> {
    // Two or more: a decision with fewer is not a decision, and recording one
    // would let a resolution name an alternative that was never considered.
    if options.len() < 2 {
        return Err(invalid(
            "options",
            "at least two options are required: a decision is a choice between them",
        ));
    }
    if options.len() > crate::core::client::MAX_TICKET_OPTIONS {
        return Err(invalid(
            "options",
            &format!(
                "at most {} options per ticket",
                crate::core::client::MAX_TICKET_OPTIONS
            ),
        ));
    }
    for option in options {
        if option.trim().is_empty() {
            return Err(invalid("options", "an empty option is not an option"));
        }
        if option.len() > crate::core::client::MAX_TICKET_OPTION_BYTES {
            return Err(invalid(
                "options",
                &format!(
                    "an option may not exceed {} bytes",
                    crate::core::client::MAX_TICKET_OPTION_BYTES
                ),
            ));
        }
    }
    Ok(())
}

fn validate_ticket_note(note: Option<&str>) -> Result<(), ClientBridgeError> {
    if let Some(note) = note
        && note.len() > crate::core::client::MAX_TICKET_NOTE_BYTES
    {
        return Err(invalid(
            "note",
            &format!(
                "a resolution note may not exceed {} bytes",
                crate::core::client::MAX_TICKET_NOTE_BYTES
            ),
        ));
    }
    Ok(())
}

fn invalid(field: &'static str, detail: &str) -> ClientBridgeError {
    ClientBridgeError::InvalidInput {
        code: ReasonCode::SchemaInvalid,
        field,
        detail: detail.to_owned(),
    }
}

/// A child-run name becomes a worktree directory and a branch name once
/// execution lands. Bounded, path-safe, and flag-safe now so the execution
/// phase never has to untangle a hostile identifier later (Phase D2).
fn validate_child_run_name(name: &str) -> Result<(), ClientBridgeError> {
    if name.trim().is_empty() {
        return Err(invalid("name", "a child-run name is required"));
    }
    if name.len() > crate::core::client::MAX_CHILD_RUN_NAME_BYTES {
        return Err(invalid("name", "a child-run name may not exceed 64 bytes"));
    }
    let starts_safely = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric());
    let body_safe = name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'));
    if !starts_safely || !body_safe || name.contains("..") {
        return Err(invalid(
            "name",
            "a child-run name may only contain ASCII letters, digits, '.', '_', '-', must \
             start with a letter or digit, and may not contain '..'",
        ));
    }
    Ok(())
}

fn validate_child_directive(directive: &str) -> Result<(), ClientBridgeError> {
    if directive.trim().is_empty() {
        return Err(invalid(
            "directive",
            "an empty directive is not a child-run task",
        ));
    }
    if directive.len() > crate::core::client::MAX_CHILD_RUN_DIRECTIVE_BYTES {
        return Err(invalid(
            "directive",
            "a child-run directive may not exceed 4096 bytes",
        ));
    }
    Ok(())
}

fn validate_base_revision(revision: &str) -> Result<(), ClientBridgeError> {
    if revision.trim().is_empty() {
        return Err(invalid("baseRevision", "a base revision is required"));
    }
    let bounded = revision.len() <= crate::core::client::MAX_BASE_REVISION_BYTES;
    let safe = revision
        .chars()
        .all(|character| !character.is_whitespace() && !character.is_control());
    if !bounded || !safe {
        return Err(invalid(
            "baseRevision",
            "a base revision is at most 256 bytes of non-whitespace text",
        ));
    }
    Ok(())
}

fn validate_clone_source(source: &str) -> Result<(), ClientBridgeError> {
    if source.trim().is_empty() {
        return Err(invalid("source", "a clone source is required"));
    }
    if source.len() > MAX_CLONE_SOURCE_BYTES {
        return Err(invalid(
            "source",
            &format!("a clone source may not exceed {MAX_CLONE_SOURCE_BYTES} bytes"),
        ));
    }
    if source.starts_with('-') || source.chars().any(char::is_control) {
        return Err(invalid(
            "source",
            "a clone source may not start with '-' or contain control characters",
        ));
    }
    Ok(())
}

fn validate_rebase_target(target: &str) -> Result<(), ClientBridgeError> {
    if target.trim().is_empty() {
        return Err(invalid("onto", "a rebase target is required"));
    }
    if target.len() > MAX_REBASE_TARGET_BYTES
        || target.starts_with('-')
        || target.chars().any(char::is_control)
    {
        return Err(invalid(
            "onto",
            "a rebase target is bounded, flag-safe, and may not contain control characters",
        ));
    }
    Ok(())
}

/// Bound the whole `paths` list, then each element. `git add -- <path>` still
/// treats a leading `-` as a flag on some argument forms, and an empty element
/// silently expands to "nothing", so both are refused rather than passed on.
fn validate_repository_paths(paths: &[String]) -> Result<(), ClientBridgeError> {
    if paths.is_empty() {
        return Err(invalid("paths", "at least one path is required"));
    }
    if paths.len() > MAX_REPOSITORY_PATHS {
        return Err(invalid(
            "paths",
            &format!("at most {MAX_REPOSITORY_PATHS} paths may be staged in one command"),
        ));
    }
    for path in paths {
        validate_repository_path(path)?;
    }
    Ok(())
}

fn validate_repository_path(path: &str) -> Result<(), ClientBridgeError> {
    if path.trim().is_empty() {
        return Err(invalid("paths", "an empty path is not a repository path"));
    }
    if path.len() > MAX_REPOSITORY_PATH_BYTES {
        return Err(invalid(
            "paths",
            &format!("a repository path may not exceed {MAX_REPOSITORY_PATH_BYTES} bytes"),
        ));
    }
    if path.starts_with('-') {
        return Err(invalid(
            "paths",
            "a repository path may not start with '-': git would read it as a flag",
        ));
    }
    if path.chars().any(char::is_control) {
        return Err(invalid(
            "paths",
            "a repository path may not contain control characters",
        ));
    }
    if path.starts_with('/') || path.split('/').any(|segment| segment == "..") {
        return Err(invalid(
            "paths",
            "a repository path is relative to the repository root and may not traverse '..'",
        ));
    }
    Ok(())
}

fn validate_workspace_file_path(path: &str) -> Result<(), ClientBridgeError> {
    if path.trim().is_empty() {
        return Err(invalid("path", "a workspace file path is required"));
    }
    if path.len() > crate::core::client::workspace::MAX_WORKSPACE_FILE_PATH_BYTES {
        return Err(invalid("path", "a workspace file path is too long"));
    }
    if path.contains('\0') {
        return Err(invalid(
            "path",
            "a workspace file path may not contain a NUL byte",
        ));
    }
    if path.starts_with('/') || path.split(['/', '\\']).any(|segment| segment == "..") {
        return Err(invalid(
            "path",
            "a workspace file path must stay relative to the project",
        ));
    }
    Ok(())
}

fn validate_save_text(text: &str) -> Result<(), ClientBridgeError> {
    if text.len() > MAX_SAVE_TEXT_BYTES {
        return Err(invalid(
            "text",
            &format!("editor text may not exceed {MAX_SAVE_TEXT_BYTES} bytes"),
        ));
    }
    if text.contains('\0') {
        return Err(invalid("text", "editor text may not contain a NUL byte"));
    }
    Ok(())
}

/// Branch names reach `git branch` and `git merge` as argv, and land in
/// `.git/refs/heads/`. `git check-ref-format`'s rules are the contract; this
/// enforces the subset mjolnr needs and refuses the rest.
fn validate_branch_name(name: &str) -> Result<(), ClientBridgeError> {
    if name.trim().is_empty() {
        return Err(invalid("name", "a branch name is required"));
    }
    if name.len() > MAX_BRANCH_NAME_BYTES {
        return Err(invalid(
            "name",
            &format!("a branch name may not exceed {MAX_BRANCH_NAME_BYTES} bytes"),
        ));
    }
    if name.starts_with('-') {
        return Err(invalid(
            "name",
            "a branch name may not start with '-': git would read it as a flag",
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid(
            "name",
            "a branch name may not contain control characters",
        ));
    }
    let forbidden = name.chars().any(|character| {
        character.is_whitespace() || matches!(character, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    });
    if forbidden
        || name.contains("..")
        || name.contains("@{")
        || name.starts_with('/')
        || name.ends_with('/')
        // `.lock` is git's own reserved suffix. Compared case-insensitively
        // because on a case-insensitive filesystem `.LOCK` collides with it.
        || name.to_ascii_lowercase().ends_with(".lock")
        || name.ends_with('.')
    {
        return Err(invalid(
            "name",
            "a branch name may not contain whitespace, '..', '@{', any of \"~^:?*[\\\\\", a \
             leading or trailing '/', or a trailing '.' or '.lock' (git check-ref-format)",
        ));
    }
    Ok(())
}

/// A commit message is human text, so it is bounded rather than
/// character-restricted — but it is never empty, because an empty message
/// makes `git commit` either abort or record nothing a reader can audit.
fn validate_commit_message(message: &str) -> Result<(), ClientBridgeError> {
    if message.trim().is_empty() {
        return Err(invalid(
            "message",
            "a commit message is required: mjolnr does not author a commit record for you",
        ));
    }
    if message.len() > MAX_COMMIT_MESSAGE_BYTES {
        return Err(invalid(
            "message",
            &format!("a commit message may not exceed {MAX_COMMIT_MESSAGE_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// An integration id selects which configured account a command reaches, so it
/// is an identifier, not free text.
fn validate_integration_id(source: &str) -> Result<(), ClientBridgeError> {
    if source.trim().is_empty() {
        return Err(invalid("source", "an integration id is required"));
    }
    if source.len() > MAX_INTEGRATION_ID_BYTES {
        return Err(invalid(
            "source",
            &format!("an integration id may not exceed {MAX_INTEGRATION_ID_BYTES} bytes"),
        ));
    }
    if !source
        .chars()
        .all(|character| character.is_ascii_lowercase() || matches!(character, '-' | '0'..='9'))
    {
        return Err(invalid(
            "source",
            "an integration id is lowercase ASCII letters, digits, and '-'",
        ));
    }
    Ok(())
}

fn validate_remote_task_id(task_id: &str) -> Result<(), ClientBridgeError> {
    if task_id.trim().is_empty() {
        return Err(invalid("taskId", "a remote task id is required"));
    }
    if task_id.len() > MAX_REMOTE_TASK_ID_BYTES {
        return Err(invalid(
            "taskId",
            &format!("a remote task id may not exceed {MAX_REMOTE_TASK_ID_BYTES} bytes"),
        ));
    }
    if task_id.chars().any(char::is_control) {
        return Err(invalid(
            "taskId",
            "a remote task id may not contain control characters",
        ));
    }
    Ok(())
}

/// Bound externally supplied text and refuse control characters.
///
/// Newlines are legitimate in an issue body, so they are kept; every other
/// control character is refused, because ANSI escapes reaching a terminal
/// client from a third party's issue title is a real path (AGENTS.md §5's
/// bounded-output rule and the D0 "no unbounded text" acceptance bullet).
fn validate_remote_text(
    field: &'static str,
    text: &str,
    limit: usize,
) -> Result<(), ClientBridgeError> {
    if text.len() > limit {
        return Err(invalid(
            field,
            &format!("remote {field} may not exceed {limit} bytes"),
        ));
    }
    if text
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(invalid(
            field,
            "remote text may not contain control characters other than newline and tab",
        ));
    }
    Ok(())
}

/// A review body is a person's remark, so it is bounded rather than
/// character-restricted. Empty is refused: an empty note says nothing, and a
/// thread with nothing in it is a marker on a line for no stated reason.
fn validate_review_body(body: &str) -> Result<(), ClientBridgeError> {
    if body.trim().is_empty() {
        return Err(invalid("body", "an empty review note says nothing"));
    }
    if body.len() > MAX_REVIEW_NOTE_BYTES {
        return Err(invalid(
            "body",
            &format!("a review note may not exceed {MAX_REVIEW_NOTE_BYTES} bytes"),
        ));
    }
    if body
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(invalid(
            "body",
            "a review note may not contain control characters other than newline and tab",
        ));
    }
    Ok(())
}

/// The digest is mjolnr's own SHA-256 hex, handed back. Requiring hex is not
/// decoration: it is the difference between echoing a value mjolnr produced and
/// accepting arbitrary text into the staleness comparison.
fn validate_capture_digest(digest: &str) -> Result<(), ClientBridgeError> {
    if digest.is_empty() || digest.len() > MAX_CAPTURE_DIGEST_BYTES {
        return Err(invalid(
            "captureDigest",
            &format!(
                "a capture digest is between 1 and {MAX_CAPTURE_DIGEST_BYTES} characters of \
                 hexadecimal"
            ),
        ));
    }
    if !digest
        .chars()
        .all(|character| character.is_ascii_hexdigit())
    {
        return Err(invalid(
            "captureDigest",
            "a capture digest is hexadecimal: pass back the value the change set carried",
        ));
    }
    Ok(())
}

fn parse_review_thread(raw: &str) -> Result<ReviewThreadId, ClientBridgeError> {
    uuid::Uuid::parse_str(raw.trim())
        .map(ReviewThreadId::from_uuid)
        .map_err(|_| invalid("threadId", "a review thread id is its UUID text form"))
}

fn parse_session(raw: &str) -> Result<SessionId, ClientBridgeError> {
    uuid::Uuid::parse_str(raw.trim())
        .map(SessionId::from_uuid)
        .map_err(|_| invalid("session", "a session id is its UUID text form"))
}

fn parse_approval(raw: &str) -> Result<ApprovalId, ClientBridgeError> {
    uuid::Uuid::parse_str(raw.trim())
        .map(ApprovalId::from_uuid)
        .map_err(|_| invalid("approval", "an approval id is its UUID text form"))
}

fn map_external_agent_command(command: &ClientCommand) -> Result<MjolnrCommand, ClientBridgeError> {
    match command {
        ClientCommand::ExternalAgentList => Err(invalid(
            "command",
            "list is a query, not a dispatch — use the snapshot",
        )),
        ClientCommand::ExternalAgentLaunch { profile } => {
            validate_external_agent_profile_name(profile)?;
            Ok(MjolnrCommand::LaunchExternalAgent {
                profile: profile.clone(),
            })
        }
        ClientCommand::ExternalAgentStop { id } => {
            let _ = parse_external_agent_id(id)?;
            Ok(MjolnrCommand::StopExternalAgent { id: id.clone() })
        }
        ClientCommand::ExternalAgentImport { id } => {
            let _ = parse_external_agent_id(id)?;
            Ok(MjolnrCommand::ImportExternalAgentChanges { id: id.clone() })
        }
        _ => Err(invalid("command", "not an external-agent command")),
    }
}

fn validate_external_agent_profile_name(name: &str) -> Result<(), ClientBridgeError> {
    if name.trim().is_empty() || name.len() > 64 {
        return Err(invalid(
            "profile",
            "external-agent profile name must be 1-64 characters",
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(invalid(
            "profile",
            "profile name may contain only lowercase letters, digits, '-' and '_'",
        ));
    }
    Ok(())
}

fn parse_external_agent_id(raw: &str) -> Result<uuid::Uuid, ClientBridgeError> {
    uuid::Uuid::parse_str(raw.trim())
        .map_err(|_| invalid("id", "an external-agent id is its UUID text form"))
}
