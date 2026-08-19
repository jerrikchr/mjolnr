//! Council configuration and structured deliberation output.
//!
//! A council convenes several models to deliberate a question or review a plan.
//! It is advisory: it runs read-only child sessions, records every turn as
//! evidence in those sessions, and produces a recommendation *with dissents
//! preserved*. It never acts in the repo — acting on its output is a separate,
//! ordinary, gated run.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// The absolute ceiling on deliberation rounds, applied on top of a config's
/// own `max_rounds` so cost is bounded by construction, not by hope.
pub const MAX_ROUNDS_CEILING: usize = 4;
/// The one project-local council configuration smed reads.
pub const COUNCIL_CONFIG_PATH: &str = ".mjolnr/council.yaml";
/// Maximum number of independently dispositionable sections in one artifact.
/// Larger documents are grouped into one bounded remainder section so the
/// council cannot turn an unbounded file into an unbounded provider fan-out.
pub const MAX_ARTIFACT_SECTIONS: usize = 16;
/// Maximum text carried into one section prompt.
pub const MAX_ARTIFACT_SECTION_CHARS: usize = 12_000;

/// Stable identity for one completed council review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CouncilReviewId(Uuid);

impl CouncilReviewId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for CouncilReviewId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CouncilReviewId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Stable identity for one finding within a council review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CouncilFindingId(Uuid);

impl CouncilFindingId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for CouncilFindingId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CouncilFindingId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Configuration for the one advisory council smed reads from
/// [`COUNCIL_CONFIG_PATH`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouncilConfig {
    /// Roles (or literal route names) whose models participate. Each resolves
    /// through the Phase 15/16 route table to a provider/model.
    pub roles: Vec<String>,
    /// Requested deliberation rounds, clamped to [`MAX_ROUNDS_CEILING`].
    pub max_rounds: usize,
    /// An optional per-turn provider-turn budget for the whole council. `None`
    /// means "run to the round cap"; a value too small to fund one turn per
    /// member per round refuses the council upfront (the Phase 13 insolvency
    /// rule), rather than discovering insolvency mid-deliberation.
    #[serde(default)]
    pub budget_provider_turns: Option<u32>,
}

impl Default for CouncilConfig {
    fn default() -> Self {
        Self {
            roles: vec!["plan".to_owned(), "code".to_owned(), "fast".to_owned()],
            max_rounds: 2,
            budget_provider_turns: None,
        }
    }
}

impl CouncilConfig {
    /// The rounds this council will actually run: its request, clamped to the
    /// structural ceiling and to at least one.
    #[must_use]
    pub fn effective_rounds(&self) -> usize {
        self.max_rounds.clamp(1, MAX_ROUNDS_CEILING)
    }
}

/// One council member's contribution across the rounds, preserved verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouncilContribution {
    /// The role (or route name) this member spoke for.
    pub role: String,
    /// The member's round-one proposal.
    pub proposal: String,
    /// The member's critique of the others, when a critique round ran.
    pub critique: Option<String>,
}

/// One member's position on a finding, preserving dissent rather than
/// replacing it with a synthesized vote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouncilMemberPosition {
    pub role: String,
    pub response: String,
    pub critique: Option<String>,
}

/// The exact artifact identity a council read. The bytes are not duplicated in
/// the event log; the digest lets a later human save path refuse a stale
/// amendment rather than silently applying it to a different file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouncilArtifact {
    pub path: String,
    pub source_digest: String,
}

/// One deterministic section of an artifact review prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CouncilArtifactSection {
    pub title: String,
    pub text: String,
}

/// A bounded, human-dispositionable council finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouncilFinding {
    pub id: CouncilFindingId,
    /// Markdown heading or the bounded fallback label for plain text.
    #[serde(default)]
    pub section: String,
    pub title: String,
    pub positions: Vec<CouncilMemberPosition>,
    pub disposition: Option<CouncilFindingDisposition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CouncilDisposition {
    Accept,
    Reject,
    Defer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouncilFindingDisposition {
    pub review_id: CouncilReviewId,
    pub finding_id: CouncilFindingId,
    pub disposition: CouncilDisposition,
    pub note: Option<String>,
    pub decided_at: OffsetDateTime,
}

/// Structured outcome of a council deliberation, assembled from real member
/// turns — never fabricated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouncilReview {
    pub review_id: CouncilReviewId,
    /// The question or plan path the council deliberated.
    pub question: String,
    /// Present when this review was launched by the generated-plan workflow.
    #[serde(default)]
    pub plan_id: Option<crate::core::plan::PlanId>,
    /// The PRD revision the linked plan review read.
    #[serde(default)]
    pub prd_id: Option<crate::core::plan::PrdId>,
    /// Each member's preserved contribution.
    pub contributions: Vec<CouncilContribution>,
    /// Rounds actually conducted.
    pub rounds_conducted: usize,
    /// Present when this review was started with `/council plan <path>`.
    #[serde(default)]
    pub artifact: Option<CouncilArtifact>,
    pub findings: Vec<CouncilFinding>,
}

/// Split a bounded text artifact at Markdown headings without interpreting its
/// prose. The council receives the exact section text, while the heading gives
/// a human a stable disposition unit. Plain text is one section.
#[must_use]
pub fn split_artifact_sections(text: &str) -> Vec<CouncilArtifactSection> {
    let mut sections = Vec::new();
    let mut title = "Artifact".to_owned();
    let mut body = String::new();

    for line in text.lines() {
        if let Some(next_title) = markdown_heading(line) {
            if body.trim().is_empty() {
                title = next_title;
            } else if sections.len() < MAX_ARTIFACT_SECTIONS.saturating_sub(1) {
                sections.push(CouncilArtifactSection {
                    title,
                    text: bounded_section_text(&body),
                });
                title = next_title;
                body.clear();
            } else {
                "Additional sections (bounded)".clone_into(&mut title);
            }
        }
        body.push_str(line);
        body.push('\n');
    }

    if !body.trim().is_empty() || sections.is_empty() {
        sections.push(CouncilArtifactSection {
            title,
            text: bounded_section_text(&body),
        });
    }
    sections
}

fn markdown_heading(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > 6 || trimmed.as_bytes().get(hashes) != Some(&b' ') {
        return None;
    }
    let title = trimmed[hashes..].trim();
    (!title.is_empty()).then(|| title.to_owned())
}

fn bounded_section_text(text: &str) -> String {
    text.chars().take(MAX_ARTIFACT_SECTION_CHARS).collect()
}

/// A deterministic, human-reviewable amendment proposal.
///
/// This is a *proposal*, never a write. It is composed by marking up the
/// artifact the council actually read with the findings a human accepted; no
/// provider turn runs to produce it and no prose is synthesized. The text is
/// handed to the ordinary editor save path, which re-checks the digest before
/// anything reaches disk — so an amendment cannot become a silent overwrite,
/// and accepting a finding still authorizes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouncilAmendment {
    pub review_id: CouncilReviewId,
    /// Workspace-relative path of the artifact the council read.
    pub path: String,
    /// The digest the amendment was composed against, carried so the save path
    /// can refuse a stale application rather than relocating the write.
    pub source_digest: String,
    /// Accepted findings actually folded into the proposal.
    pub accepted_findings: usize,
    /// The proposed artifact text, for a human to read and edit before saving.
    pub text: String,
}

/// Marker opening an amendment block, chosen so it survives a Markdown render
/// as visible text rather than disappearing into the document's own prose.
const AMENDMENT_MARKER: &str = "<!-- smed:council-amendment -->";

impl CouncilReview {
    /// Compose a human-reviewable amended artifact from the findings a human
    /// accepted.
    ///
    /// Refuses when the review reviewed no artifact, when the file on disk has
    /// moved since the council read it, or when nothing has been accepted —
    /// each refusal being a case where producing text would imply a judgement
    /// no human made.
    pub fn propose_amendment(
        &self,
        original_text: &str,
        observed_digest: &str,
    ) -> Result<CouncilAmendment, String> {
        let artifact = self.artifact.as_ref().ok_or_else(|| {
            "This council reviewed a question rather than an artifact, so there is nothing to amend"
                .to_owned()
        })?;
        if artifact.path.starts_with("prd://") {
            return Err(
                "generated PRDs are durable runtime artifacts and cannot be amended through a file save"
                    .to_owned(),
            );
        }
        if observed_digest != artifact.source_digest {
            return Err(format!(
                "`{}` has changed since the council read it, so the amendment was not composed; re-run the council against the current file",
                artifact.path
            ));
        }

        let accepted: Vec<&CouncilFinding> = self
            .findings
            .iter()
            .filter(|finding| {
                finding.disposition.as_ref().is_some_and(|disposition| {
                    disposition.disposition == CouncilDisposition::Accept
                })
            })
            .collect();
        if accepted.is_empty() {
            return Err(
                "No finding has been accepted, so there is nothing to fold into an amendment"
                    .to_owned(),
            );
        }

        let text = Self::render_amendment(original_text, &accepted);
        Ok(CouncilAmendment {
            review_id: self.review_id,
            path: artifact.path.clone(),
            source_digest: artifact.source_digest.clone(),
            accepted_findings: accepted.len(),
            text,
        })
    }

    /// Walk the original text, emitting it unchanged and appending each
    /// section's accepted findings as a marked block when that section ends.
    /// The artifact's own bytes are never rewritten — a human does that in the
    /// editor, which is the point.
    fn render_amendment(original_text: &str, accepted: &[&CouncilFinding]) -> String {
        let mut out = String::with_capacity(original_text.len());
        let mut emitted: Vec<&CouncilFindingId> = Vec::new();
        let mut section = String::new();

        for line in original_text.lines() {
            if let Some(next_title) = markdown_heading(line) {
                append_section_amendments(&mut out, accepted, &section, &mut emitted);
                section = next_title;
            }
            out.push_str(line);
            out.push('\n');
        }
        append_section_amendments(&mut out, accepted, &section, &mut emitted);

        // Findings whose section never matched a heading — the bounded
        // remainder, or a renamed heading — are appended rather than dropped,
        // because silently losing an accepted finding is the one outcome a
        // human cannot see.
        let unmatched: Vec<&&CouncilFinding> = accepted
            .iter()
            .filter(|finding| !emitted.contains(&&finding.id))
            .collect();
        if !unmatched.is_empty() {
            out.push_str("\n## Council amendments (unmatched sections)\n");
            for finding in unmatched {
                append_finding(&mut out, finding);
            }
        }
        out
    }
}

fn append_section_amendments<'a>(
    out: &mut String,
    accepted: &[&'a CouncilFinding],
    section: &str,
    emitted: &mut Vec<&'a CouncilFindingId>,
) {
    if section.is_empty() {
        return;
    }
    for finding in accepted.iter().filter(|finding| finding.section == section) {
        append_finding(out, finding);
        emitted.push(&finding.id);
    }
}

fn append_finding(out: &mut String, finding: &CouncilFinding) {
    use std::fmt::Write as _;

    // Writing to a String is infallible.
    let _ = writeln!(
        out,
        "\n{AMENDMENT_MARKER}\n> **Accepted finding — {}**",
        finding.title
    );
    if let Some(note) = finding
        .disposition
        .as_ref()
        .and_then(|disposition| disposition.note.as_deref())
        .filter(|note| !note.trim().is_empty())
    {
        for line in note.lines() {
            let _ = writeln!(out, "> Human note: {line}");
        }
    }
    for position in &finding.positions {
        let _ = writeln!(out, ">\n> *[{}]*", position.role);
        for line in position.response.lines() {
            let _ = writeln!(out, "> {line}");
        }
        if let Some(critique) = position.critique.as_deref() {
            let _ = writeln!(out, ">\n> *[{} — dissent preserved]*", position.role);
            for line in critique.lines() {
                let _ = writeln!(out, "> {line}");
            }
        }
    }
    out.push_str(
        "> \n> smed marked this up; it did not rewrite the section. Edit this block into the document and save it yourself.\n",
    );
}

impl CouncilReview {
    /// Render the review as the evidenced text appended to the parent session.
    /// Dissent is surfaced, never buried under a single synthesized answer.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = format!(
            "SMED COUNCIL // {} member(s), {} round(s)\n\nQuestion: {}\n",
            self.contributions.len(),
            self.rounds_conducted,
            self.question
        );
        out.push_str("\n### Proposals\n");
        for contribution in &self.contributions {
            // Writing to a String is infallible.
            let _ = write!(
                out,
                "\n[{}]\n{}\n",
                contribution.role, contribution.proposal
            );
        }
        let critiques: Vec<&CouncilContribution> = self
            .contributions
            .iter()
            .filter(|contribution| contribution.critique.is_some())
            .collect();
        if !critiques.is_empty() {
            out.push_str("\n### Dissents & critiques (preserved)\n");
            for contribution in critiques {
                if let Some(critique) = &contribution.critique {
                    let _ = write!(out, "\n[{}]\n{critique}\n", contribution.role);
                }
            }
        }
        out.push_str(
            "\nThe council is advisory. Acting on it is a separate, ordinary, gated run.\n",
        );
        out
    }

    /// Apply a human disposition to one finding without changing the finding's
    /// model-authored evidence.
    pub fn apply_disposition(
        &mut self,
        disposition: CouncilFindingDisposition,
    ) -> Result<(), String> {
        if disposition.review_id != self.review_id {
            return Err("council disposition names a different review".to_owned());
        }
        let finding = self
            .findings
            .iter_mut()
            .find(|finding| finding.id == disposition.finding_id)
            .ok_or_else(|| "council disposition names an unknown finding".to_owned())?;
        finding.disposition = Some(disposition);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CouncilArtifact, CouncilDisposition, CouncilFinding, CouncilFindingDisposition,
        CouncilFindingId, CouncilMemberPosition, CouncilReview, CouncilReviewId,
    };
    use time::OffsetDateTime;

    const ARTIFACT: &str = "# Goal\nship it\n\n## Risk\nslow\n";
    const DIGEST: &str = "abc123";

    fn review_with(dispositions: &[(&str, Option<CouncilDisposition>)]) -> CouncilReview {
        let review_id = CouncilReviewId::new();
        let findings = dispositions
            .iter()
            .map(|(section, disposition)| {
                let finding_id = CouncilFindingId::new();
                CouncilFinding {
                    id: finding_id,
                    section: (*section).to_owned(),
                    title: format!("{section} finding"),
                    positions: vec![CouncilMemberPosition {
                        role: "plan".to_owned(),
                        response: "tighten the wording".to_owned(),
                        critique: Some("but the risk is overstated".to_owned()),
                    }],
                    disposition: disposition.map(|disposition| CouncilFindingDisposition {
                        review_id,
                        finding_id,
                        disposition,
                        note: Some("agreed".to_owned()),
                        decided_at: OffsetDateTime::UNIX_EPOCH,
                    }),
                }
            })
            .collect();
        CouncilReview {
            review_id,
            question: "plan.md".to_owned(),
            plan_id: None,
            prd_id: None,
            contributions: Vec::new(),
            rounds_conducted: 1,
            artifact: Some(CouncilArtifact {
                path: "plan.md".to_owned(),
                source_digest: DIGEST.to_owned(),
            }),
            findings,
        }
    }

    #[test]
    fn amendment_marks_up_accepted_findings_and_preserves_the_original_text() {
        let review = review_with(&[
            ("Goal", Some(CouncilDisposition::Accept)),
            ("Risk", Some(CouncilDisposition::Reject)),
        ]);

        let amendment = review
            .propose_amendment(ARTIFACT, DIGEST)
            .expect("accepted finding composes");

        assert_eq!(amendment.accepted_findings, 1);
        // Every original line survives; smed marks up rather than rewrites.
        for line in ARTIFACT.lines() {
            assert!(amendment.text.contains(line), "lost original line: {line}");
        }
        assert!(amendment.text.contains("Accepted finding — Goal finding"));
        assert!(amendment.text.contains("dissent preserved"));
        // The rejected finding is not folded in.
        assert!(!amendment.text.contains("Risk finding"));
    }

    #[test]
    fn amendment_refuses_when_the_artifact_moved_under_it() {
        let review = review_with(&[("Goal", Some(CouncilDisposition::Accept))]);

        let error = review
            .propose_amendment(ARTIFACT, "a-different-digest")
            .expect_err("stale artifact must refuse");

        assert!(error.contains("has changed since the council read it"));
    }

    #[test]
    fn amendment_refuses_when_nothing_was_accepted() {
        let review = review_with(&[("Goal", Some(CouncilDisposition::Defer)), ("Risk", None)]);

        let error = review
            .propose_amendment(ARTIFACT, DIGEST)
            .expect_err("no acceptance must refuse");

        assert!(error.contains("No finding has been accepted"));
    }

    #[test]
    fn amendment_refuses_a_review_that_read_no_artifact() {
        let mut review = review_with(&[("Goal", Some(CouncilDisposition::Accept))]);
        review.artifact = None;

        let error = review
            .propose_amendment(ARTIFACT, DIGEST)
            .expect_err("questions have no artifact to amend");

        assert!(error.contains("reviewed a question"));
    }

    #[test]
    fn amendment_appends_findings_whose_section_matches_no_heading() {
        let review = review_with(&[(
            "Additional sections (bounded)",
            Some(CouncilDisposition::Accept),
        )]);

        let amendment = review
            .propose_amendment(ARTIFACT, DIGEST)
            .expect("unmatched sections still compose");

        assert!(
            amendment
                .text
                .contains("## Council amendments (unmatched sections)")
        );
        assert!(
            amendment
                .text
                .contains("Accepted finding — Additional sections (bounded) finding")
        );
    }
}

#[cfg(test)]
mod section_tests {
    use super::{MAX_ARTIFACT_SECTIONS, split_artifact_sections};

    #[test]
    fn artifact_sections_preserve_headings_and_plain_text() {
        let sections = split_artifact_sections("# Goal\nship it\n\n## Risk\nslow");

        assert_eq!(sections.len(), 2);
        let first = sections.first().expect("first section");
        let second = sections.get(1).expect("second section");
        assert_eq!(first.title, "Goal");
        assert!(first.text.contains("ship it"));
        assert_eq!(second.title, "Risk");
        assert!(second.text.contains("slow"));
    }

    #[test]
    fn artifact_sections_bound_heading_fan_out() {
        let text = (0..(MAX_ARTIFACT_SECTIONS + 4))
            .map(|index| format!("# Section {index}\nbody"))
            .collect::<Vec<_>>()
            .join("\n");

        let sections = split_artifact_sections(&text);

        assert_eq!(sections.len(), MAX_ARTIFACT_SECTIONS);
        assert_eq!(
            sections.last().map(|section| section.title.as_str()),
            Some("Additional sections (bounded)")
        );
    }
}
