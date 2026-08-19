//! Anchoring, replaying, and framing review threads (Phase D3 producer).
//!
//! One reason to change: how a human's line note is pinned to a diff, folded
//! into session state, and turned into a directive smed can act on.
//!
//! The division of labour matters. `core::review` holds the types and can only
//! *compare* an anchor against a digest. This module is the only place that
//! *builds* an anchor, and it builds it from the runtime's own capture — the
//! hunk header and the digest are read out of what smed captured, never taken
//! from the client that asked. A client says "path, side, line, and the digest
//! I was looking at"; everything else on the anchor is smed's own record of
//! that diff. A client that could supply its own hunk context could describe a
//! diff that never existed.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use time::OffsetDateTime;

use crate::core::change_capture::{ChangeView, FileChange, Hunk};
use crate::core::error::{ReasonCode, SmedError};
use crate::core::event::SmedEvent;
use crate::core::review::{
    ReviewAnchor, ReviewComment, ReviewSide, ReviewThread, ReviewThreadId, ReviewThreadStatus,
};

/// Threads in the order they were opened.
///
/// [`ReviewThreadId`] is a `UUIDv7`, so the map's own ordering is creation order —
/// a review surface renders them oldest-first without carrying a second index,
/// and a snapshot cannot reorder itself between publishes.
pub(crate) type ReviewThreads = BTreeMap<ReviewThreadId, ReviewThread>;

/// Pin a note to a line of the capture smed currently holds.
///
/// Every refusal here is one §D3 asks for by name, and each is a refusal rather
/// than an approximation:
///
/// - **The diff moved.** `digest` is the revision the human was looking at. If
///   the capture has moved on, the note is refused with `WORKSPACE_STALE_DIFF`
///   — §D3's "a diff whose base changed is marked stale and cannot accept a
///   line note as if current". Sliding it onto the current line at the same
///   number is exactly the silent move the contract forbids.
/// - **The file is not in this diff.** A note anchored to a file the capture
///   does not show has no line to be about.
/// - **The line is not in a hunk on that side.** Anchoring to a line the diff
///   never printed would record a position no reader could find.
pub(crate) fn anchor_note(
    changes: &ChangeView,
    path: &str,
    side: ReviewSide,
    line: u32,
    digest: &str,
) -> Result<ReviewAnchor, SmedError> {
    let Some(capture) = changes.capture() else {
        return Err(SmedError::workspace_refused(
            ReasonCode::WorkspaceCapabilityUnavailable,
            "No diff has been captured for the open project, so there is no line to anchor a \
             review note to; nothing was recorded",
        ));
    };

    if capture.digest != digest {
        return Err(SmedError::workspace_refused(
            ReasonCode::WorkspaceStaleDiff,
            format!(
                "This note was written against diff revision {digest}, and the working tree has \
                 since been captured at {}. The note was refused rather than moved to whatever \
                 is on line {line} now",
                capture.digest
            ),
        ));
    }

    let Some(file) = capture.files.iter().find(|file| file.path == path) else {
        return Err(SmedError::workspace_refused(
            ReasonCode::SchemaInvalid,
            format!("'{path}' is not a file in the captured diff, so it has no line {line}"),
        ));
    };

    let Some(hunk) = hunk_containing(file, side, line) else {
        return Err(SmedError::workspace_refused(
            ReasonCode::SchemaInvalid,
            format!(
                "line {line} on the {} side of '{path}' is not in any hunk of the captured diff",
                side.as_str()
            ),
        ));
    };

    Ok(ReviewAnchor {
        path: file.path.clone(),
        side,
        line,
        // smed's own record of the context, not the client's claim about it.
        hunk_header: hunk.header.clone(),
        capture_digest: capture.digest.clone(),
        base_object_id: capture.base_revision.clone(),
    })
}

/// The hunk that actually printed this line on this side.
///
/// Searched by the line numbers the diff assigned rather than by the hunk's
/// declared range: a hunk header states a start and a count, and trusting the
/// arithmetic instead of the lines would anchor to a line the hunk skipped.
fn hunk_containing(file: &FileChange, side: ReviewSide, line: u32) -> Option<&Hunk> {
    file.hunks.iter().find(|hunk| {
        hunk.lines.iter().any(|printed| {
            let numbered = match side {
                ReviewSide::Old => printed.old_line_number,
                ReviewSide::New => printed.new_line_number,
            };
            numbered == Some(line)
        })
    })
}

/// Fold one durable review event into the thread set.
///
/// The single reducer, called from the live path after a successful append and
/// from recovery's replay. One function rather than two so a resumed session
/// and a live one cannot drift: a divergence here would show a different set of
/// notes before and after a restart, which is the failure §D3's "notes survive
/// restart" bullet is about.
///
/// A comment or an answer naming a thread that does not exist is dropped rather
/// than creating a stub. A thread with no anchor is not a thread, and inventing
/// one to hang a comment from would put a note on the surface pointing at no
/// line at all.
pub(crate) fn apply_event(threads: &mut ReviewThreads, event: &SmedEvent) {
    match event {
        SmedEvent::ReviewNoteRecorded {
            thread,
            anchor,
            comment,
            ..
        } => {
            threads
                .entry(*thread)
                .or_insert_with(|| ReviewThread::open(*thread, anchor.clone(), comment.clone()));
        }
        SmedEvent::ReviewCommentAdded {
            thread, comment, ..
        } => {
            if let Some(existing) = threads.get_mut(thread) {
                existing.comments.push(comment.clone());
            }
        }
        SmedEvent::ReviewRequestSent { threads: sent, .. } => {
            for id in sent {
                if let Some(existing) = threads.get_mut(id) {
                    existing.status = ReviewThreadStatus::Sent;
                }
            }
        }
        SmedEvent::ReviewRequestAnswered {
            threads: answered,
            response_message,
            ..
        } => {
            for id in answered {
                if let Some(existing) = threads.get_mut(id) {
                    existing.response_message_id = Some(response_message.to_string());
                }
            }
        }
        _ => {}
    }
}

/// Turn the selected threads into the directive "send to smed" delivers.
///
/// The text is assembled from smed's own record — the anchors it built and the
/// bodies a human typed, both already bounded at the bridge — so nothing here
/// can grow without limit. It reads as a request rather than an instruction to
/// a tool because that is what it is: the human is asking for a revision, and
/// the ordinary agent loop decides what to do about it under the ordinary
/// gates. No approval, policy, or budget is touched.
pub(crate) fn request_text(threads: &[&ReviewThread]) -> String {
    let mut text = String::from(
        "Review notes on the working-tree diff. Each note is pinned to a line of the diff you \
         can re-read; address them or explain why not.\n",
    );
    for thread in threads {
        let anchor = &thread.anchor;
        let _ = writeln!(
            text,
            "\n[{}] {}:{} ({} side, {})",
            thread.id,
            anchor.path,
            anchor.line,
            anchor.side.as_str(),
            anchor.hunk_header
        );
        for comment in &thread.comments {
            text.push_str("  ");
            text.push_str(&comment.body);
            text.push('\n');
        }
    }
    text
}

/// A comment stamped now. One place, so a live comment and a replayed one carry
/// the same shape.
pub(crate) fn comment(body: String) -> ReviewComment {
    ReviewComment {
        body,
        created_at: OffsetDateTime::now_utc(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

    use super::*;
    use crate::core::change_capture::{ChangeCapture, ChangeStatus, HunkLine, LineSide};
    use crate::core::event::{RunId, SessionId};

    fn capture(digest: &str) -> ChangeView {
        ChangeView::Captured(Box::new(ChangeCapture {
            base_revision: Some("abc123".to_owned()),
            index_revision: Some("tree789".to_owned()),
            digest: digest.to_owned(),
            files: vec![FileChange {
                path: "src/main.rs".to_owned(),
                old_path: None,
                status: ChangeStatus::Modified,
                hunks: vec![Hunk {
                    old_start: 40,
                    old_lines: 2,
                    new_start: 40,
                    new_lines: 3,
                    header: "@@ -40,2 +40,3 @@".to_owned(),
                    lines: vec![
                        HunkLine {
                            kind: LineSide::Unchanged,
                            content: "context".to_owned(),
                            old_line_number: Some(40),
                            new_line_number: Some(40),
                        },
                        HunkLine {
                            kind: LineSide::Removed,
                            content: "gone".to_owned(),
                            old_line_number: Some(41),
                            new_line_number: None,
                        },
                        HunkLine {
                            kind: LineSide::Added,
                            content: "fresh".to_owned(),
                            old_line_number: None,
                            new_line_number: Some(41),
                        },
                    ],
                }],
                binary: false,
                undecodable: false,
                truncated: false,
            }],
            output_truncated: false,
            undiffed_untracked: Vec::new(),
            capture_sequence: 5,
        }))
    }

    #[test]
    fn an_anchor_takes_its_hunk_context_from_smeds_capture() {
        let anchor = anchor_note(&capture("d1"), "src/main.rs", ReviewSide::New, 41, "d1")
            .expect("a live anchor");

        assert_eq!(anchor.hunk_header, "@@ -40,2 +40,3 @@");
        assert_eq!(anchor.capture_digest, "d1");
        assert_eq!(anchor.base_object_id.as_deref(), Some("abc123"));
        assert_eq!(anchor.line, 41);
        assert_eq!(anchor.side, ReviewSide::New);
    }

    /// §D3's acceptance bullet, at the only door that creates an anchor: a
    /// diff whose revision has moved cannot accept a line note as if current.
    /// The refusal names both revisions, because "stale" without them is a
    /// dead end for whoever has to decide what to do next.
    #[test]
    fn a_note_against_a_moved_diff_is_refused_not_relocated() {
        let error = anchor_note(&capture("d2"), "src/main.rs", ReviewSide::New, 41, "d1")
            .expect_err("a stale digest must refuse");

        assert_eq!(error.reason_code(), Some(ReasonCode::WorkspaceStaleDiff));
        let message = error.to_string();
        assert!(
            message.contains("d1") && message.contains("d2"),
            "{message}"
        );
    }

    #[test]
    fn a_note_on_a_file_outside_the_diff_is_refused() {
        let error = anchor_note(&capture("d1"), "src/other.rs", ReviewSide::New, 41, "d1")
            .expect_err("an unknown path must refuse");
        assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));
    }

    /// The side is load-bearing. Line 41 exists on both sides of this hunk and
    /// they are different lines; line 42 exists on neither, and asking for it
    /// is refused rather than rounded to the nearest printed line.
    #[test]
    fn a_line_the_diff_never_printed_on_that_side_is_refused() {
        assert!(anchor_note(&capture("d1"), "src/main.rs", ReviewSide::Old, 41, "d1").is_ok());
        assert!(anchor_note(&capture("d1"), "src/main.rs", ReviewSide::New, 41, "d1").is_ok());

        let error = anchor_note(&capture("d1"), "src/main.rs", ReviewSide::New, 42, "d1")
            .expect_err("an unprinted line must refuse");
        assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));
    }

    #[test]
    fn no_capture_means_no_anchor() {
        let error = anchor_note(
            &ChangeView::NoProject,
            "src/main.rs",
            ReviewSide::New,
            41,
            "d1",
        )
        .expect_err("no capture must refuse");
        assert_eq!(
            error.reason_code(),
            Some(ReasonCode::WorkspaceCapabilityUnavailable)
        );
    }

    fn recorded(id: ReviewThreadId, session: SessionId) -> SmedEvent {
        SmedEvent::ReviewNoteRecorded {
            session,
            thread: id,
            anchor: anchor_note(&capture("d1"), "src/main.rs", ReviewSide::New, 41, "d1").unwrap(),
            comment: comment("handle the None case".to_owned()),
        }
    }

    #[test]
    fn the_reducer_builds_a_thread_and_then_moves_it_through_its_two_states() {
        let session = SessionId::new();
        let id = ReviewThreadId::new();
        let mut threads = ReviewThreads::new();

        apply_event(&mut threads, &recorded(id, session));
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[&id].status, ReviewThreadStatus::Open);
        assert!(threads[&id].response_message_id.is_none());

        apply_event(
            &mut threads,
            &SmedEvent::ReviewCommentAdded {
                session,
                thread: id,
                comment: comment("and the empty case".to_owned()),
            },
        );
        assert_eq!(threads[&id].comments.len(), 2);

        let run = RunId::new();
        apply_event(
            &mut threads,
            &SmedEvent::ReviewRequestSent {
                session,
                threads: vec![id],
                run,
            },
        );
        assert_eq!(threads[&id].status, ReviewThreadStatus::Sent);

        let answer = uuid::Uuid::now_v7();
        apply_event(
            &mut threads,
            &SmedEvent::ReviewRequestAnswered {
                session,
                threads: vec![id],
                response_message: answer,
            },
        );
        assert_eq!(
            threads[&id].response_message_id.as_deref(),
            Some(answer.to_string().as_str())
        );
        // Answering is not addressing. The status stays where the record can
        // support it: a request was sent and a reply arrived.
        assert_eq!(threads[&id].status, ReviewThreadStatus::Sent);
    }

    /// A comment or an answer for a thread that was never opened is dropped.
    /// The alternative — a stub thread — would put a note on the review surface
    /// anchored to no line at all.
    #[test]
    fn an_event_for_an_unknown_thread_does_not_conjure_one() {
        let session = SessionId::new();
        let mut threads = ReviewThreads::new();

        apply_event(
            &mut threads,
            &SmedEvent::ReviewCommentAdded {
                session,
                thread: ReviewThreadId::new(),
                comment: comment("orphan".to_owned()),
            },
        );
        apply_event(
            &mut threads,
            &SmedEvent::ReviewRequestAnswered {
                session,
                threads: vec![ReviewThreadId::new()],
                response_message: uuid::Uuid::now_v7(),
            },
        );

        assert!(threads.is_empty());
    }

    /// Replaying the same note twice — a live append followed by a recovery
    /// replay of the same event — must not double the thread or its comments.
    #[test]
    fn replaying_a_recorded_note_is_idempotent() {
        let session = SessionId::new();
        let id = ReviewThreadId::new();
        let mut threads = ReviewThreads::new();

        apply_event(&mut threads, &recorded(id, session));
        apply_event(&mut threads, &recorded(id, session));

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[&id].comments.len(), 1);
    }

    #[test]
    fn the_request_names_every_thread_its_line_and_its_body() {
        let session = SessionId::new();
        let id = ReviewThreadId::new();
        let mut threads = ReviewThreads::new();
        apply_event(&mut threads, &recorded(id, session));

        let text = request_text(&[&threads[&id]]);
        assert!(text.contains(&id.to_string()));
        assert!(text.contains("src/main.rs:41"));
        assert!(text.contains("new side"));
        assert!(text.contains("@@ -40,2 +40,3 @@"));
        assert!(text.contains("handle the None case"));
    }
}
