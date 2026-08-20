//! Anchored review threads (Phase D3).
//!
//! A review thread is a human note pinned to one line of one diff. §D3 requires
//! the anchor to name a file, a side, a line, hunk context, **and a diff
//! revision**, and requires that "stale anchors remain visible but cannot
//! silently move to a different line". Both properties live in the shape of
//! [`ReviewAnchor`]: every field is recorded once, at the moment the note was
//! taken, and nothing in this module can recompute one later. A thread has no
//! method that resolves its line against a newer capture, because the only way
//! to slide a note onto the wrong code is to have written one.
//!
//! # What this module deliberately cannot express
//!
//! - **A resolved, applied, or verified thread.** [`ReviewThreadStatus`] has two
//!   variants and neither is a claim about the code. mjolnr cannot know whether a
//!   note was addressed — only that it was written, and that a request carrying
//!   it was sent — so there is nowhere to record that it was (AGENTS.md §1.3).
//!   `no_status_claims_the_change_was_addressed` pins that.
//! - **A comment mjolnr wrote.** [`ReviewComment`] has no author field because
//!   every comment is a human act. mjolnr answers a review request in the
//!   transcript, as an ordinary message, and the thread links to that message by
//!   id. A thread that could hold a model-authored comment would let the review
//!   surface show mjolnr agreeing with itself.
//! - **A thread with no anchor.** There is no free-floating note: the anchor is
//!   a required field, and the runtime derives its hunk context and digest from
//!   its own capture rather than accepting them from a client.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Identifies one review thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReviewThreadId(Uuid);

impl ReviewThreadId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Rebuild an id read from durable history or sent by a client.
    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for ReviewThreadId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ReviewThreadId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Which side of the diff a note is pinned to.
///
/// Not derivable from the line number: a removed line and an added line can
/// carry the same number on their own sides, and a note on "line 12" is a
/// different note depending on which twelve was meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewSide {
    /// The pre-image: a context or removed line, numbered in the old file.
    Old,
    /// The post-image: a context or added line, numbered in the new file.
    New,
}

impl ReviewSide {
    /// The stable wire spelling. Contract, like a reason code (AGENTS.md §6).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Old => "old",
            Self::New => "new",
        }
    }
}

/// Where a note is pinned, recorded once and never recomputed.
///
/// `capture_digest` is the diff revision §D3 asks for: a SHA-256 over the exact
/// diff bytes the human was looking at. It moves whenever the working tree does,
/// which `base_object_id` alone cannot see — HEAD can sit still while the file
/// under review changes underneath the note. Comparing it against the current
/// capture is the whole staleness test, and a mismatch makes the anchor stale
/// rather than making it move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewAnchor {
    pub path: String,
    pub side: ReviewSide,
    pub line: u32,
    /// The hunk header exactly as the capture printed it — §D3's "hunk
    /// context". Derived by the runtime from its own capture, never accepted
    /// from a client, so a note cannot claim a context the diff did not have.
    pub hunk_header: String,
    /// The diff revision this note was taken against.
    pub capture_digest: String,
    /// The commit the diff was taken from, when there was one. Carried for a
    /// reader; the staleness test uses `capture_digest`.
    pub base_object_id: Option<String>,
}

/// One human remark on a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub body: String,
    pub created_at: OffsetDateTime,
}

/// How far a thread has travelled. Neither variant is a claim about the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewThreadStatus {
    /// Written down. mjolnr has not been asked to do anything about it.
    Open,
    /// A durable revision request naming this thread was sent into the session.
    Sent,
}

impl ReviewThreadStatus {
    /// The stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Sent => "sent",
        }
    }
}

/// A note and everything said on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewThread {
    pub id: ReviewThreadId,
    pub anchor: ReviewAnchor,
    pub comments: Vec<ReviewComment>,
    pub status: ReviewThreadStatus,
    /// The message mjolnr answered with, once a sent request produced one.
    ///
    /// A `CanonicalMessage` id, which is what a client already keys its
    /// transcript by — so "link to the resulting mjolnr response" is a link the
    /// surface can actually follow, not a run identifier no rendered message
    /// carries. `None` until an answer exists; a run that was cancelled or
    /// failed leaves it `None`, because there is no response to point at.
    pub response_message_id: Option<String>,
}

impl ReviewThread {
    /// Open a thread with its first comment.
    #[must_use]
    pub fn open(id: ReviewThreadId, anchor: ReviewAnchor, first: ReviewComment) -> Self {
        Self {
            id,
            anchor,
            comments: vec![first],
            status: ReviewThreadStatus::Open,
            response_message_id: None,
        }
    }

    /// Whether this note was taken against a diff revision other than `digest`.
    ///
    /// The only question this type answers about the current tree, and it is a
    /// comparison, not a relocation: a stale thread keeps the line it recorded.
    #[must_use]
    pub fn is_stale_against(&self, digest: &str) -> bool {
        self.anchor.capture_digest != digest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor() -> ReviewAnchor {
        ReviewAnchor {
            path: "src/main.rs".to_owned(),
            side: ReviewSide::New,
            line: 42,
            hunk_header: "@@ -40,7 +40,9 @@".to_owned(),
            capture_digest: "digest-one".to_owned(),
            base_object_id: Some("abc123".to_owned()),
        }
    }

    fn thread() -> ReviewThread {
        ReviewThread::open(
            ReviewThreadId::new(),
            anchor(),
            ReviewComment {
                body: "handle the None case".to_owned(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            },
        )
    }

    #[test]
    fn a_thread_round_trips() {
        let thread = thread();
        let json = serde_json::to_string(&thread).unwrap();
        let parsed: ReviewThread = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, thread);
    }

    /// §D3: "stale anchors remain visible but cannot silently move to a
    /// different line." Staleness is a comparison; the recorded line, side, and
    /// hunk header are untouched by it. Nothing on this type can rewrite them,
    /// and this is where a future `resolve_against` would have to argue for
    /// itself.
    #[test]
    fn a_stale_anchor_keeps_the_line_it_was_taken_against() {
        let thread = thread();
        assert!(!thread.is_stale_against("digest-one"));
        assert!(thread.is_stale_against("digest-two"));

        assert_eq!(thread.anchor.line, 42);
        assert_eq!(thread.anchor.side, ReviewSide::New);
        assert_eq!(thread.anchor.hunk_header, "@@ -40,7 +40,9 @@");
        assert_eq!(thread.anchor.capture_digest, "digest-one");
    }

    /// The false-promotion guard §D3 asks for, at the type level. A thread may
    /// say it was written and that a request naming it was sent. It may not say
    /// the change was applied, imported, or verified — mjolnr does not know that,
    /// and a status string a surface could render as "done" would be the lie.
    #[test]
    fn no_status_claims_the_change_was_addressed() {
        for status in [ReviewThreadStatus::Open, ReviewThreadStatus::Sent] {
            let wire = serde_json::to_string(&status).unwrap();
            for forbidden in [
                "resolved", "applied", "verified", "fixed", "done", "imported", "proposed",
            ] {
                assert!(
                    !wire.contains(forbidden),
                    "a review status must not claim the change was addressed, found \
                     {forbidden} in {wire}"
                );
            }
        }
        // The inverse half: an invented status must not deserialize. A frontend
        // that sends `"verified"` is refused at the wire, not filed under a
        // catch-all.
        assert!(serde_json::from_str::<ReviewThreadStatus>("\"verified\"").is_err());
        assert!(serde_json::from_str::<ReviewThreadStatus>("\"resolved\"").is_err());
    }

    #[test]
    fn a_side_is_explicit_because_a_line_number_cannot_imply_one() {
        assert_eq!(ReviewSide::Old.as_str(), "old");
        assert_eq!(ReviewSide::New.as_str(), "new");
        assert_eq!(
            serde_json::to_string(&ReviewSide::New).unwrap(),
            "\"new\"".to_owned()
        );
        assert!(serde_json::from_str::<ReviewSide>("\"both\"").is_err());
    }

    /// A comment carries no author, so a model-authored remark cannot be filed
    /// as one. mjolnr answers in the transcript and the thread links to that
    /// message; the review surface never shows mjolnr agreeing with itself.
    #[test]
    fn a_comment_has_nowhere_to_record_a_model_author() {
        let rendered = format!("{:?}", thread().comments);
        assert!(!rendered.contains("author") && !rendered.contains("role"));
    }
}
