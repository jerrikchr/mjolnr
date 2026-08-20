# ADR-0015: A decision ticket's resolution records judgement, and carries no authority

**Status:** Accepted
**Date:** 2026-08-04
**Deciders:** Jerrik Christiansen
**Phase:** E5

## Context

The open question this ADR settles: "What does a decision ticket's resolution
record, such that it is durable human judgement without becoming an
authorisation?"

The constraint had been stated three times across the planning documents and the
phase plan, in the same words each time: a decision ticket's *resolution* is a
durable record of a human decision; it is **not** an authorisation, and the two
must not be allowed to blur.

Saying it three times does not make it structural. This ADR makes it structural.

mjolnr already has a working precedent: `CouncilFindingDisposition` in
`src/core/council.rs`. A human records
accept, reject, or defer on one council finding; it is durable, replayable, and
provably not an approval, because the disposition type and the approval types
(`PlanApproval`, the tool gate, `PolicyMode`) are different types and nothing
reads one as the other. The E6 amendment path then had to *re-derive* everything
it needed from the artifact and the digest — an accepted finding funded no write
on its own.

## Decision

**A decision ticket's resolution records: the question, the options considered,
the option chosen, the human who chose it, when, and an optional note. It carries
no field that any executor can branch on.**

It is modelled on `CouncilFindingDisposition` deliberately, and it obeys the same
three structural rules:

1. **A resolution is its own type.** It is not a variant of `PlanApproval`, does
   not embed a `PolicyMode`, and does not appear in any signature that grants
   capability. The type system carries the constraint, not a comment.
2. **A resolution moves the frontier; it never widens a policy.** Resolving a
   ticket changes what is *decidable* — it can unblock other tickets in the
   blocking graph. It cannot change what is *permitted*. Those are different
   questions and, after this ADR, different code paths.
3. **Resolutions are permanent and additive.** A resolution is recorded once and
   is long-lived. Changing your mind records a new resolution that supersedes the
   old one by reference; it does not mutate or delete it, because the reasoning
   behind a superseded decision is the thing that stops it being re-litigated
   from scratch.

## The trap this exists to avoid

**There must be no `status: approved` on a resolution.**

The moment a resolution carries a word an executor could branch on, something
eventually branches on it, and the ticket becomes an authorisation by usage even
though it was never one by design. That is the same failure the council design
rejects — "2/3 quorum reached, approved" decides, and a model must not — and the
same one ADR-0014 keeps out of the frontier computation.

The chosen option is therefore recorded as **a reference to one of the options
considered**, not as a status word. "The owner chose option B" is a fact about a
decision. "This ticket is approved" is a claim about permission, and the second
is not a thing a decision ticket is entitled to say.

Concretely, and for the same reason `CouncilFindingDisposition` works: the
resolution names the *question* it answers, so reading it out of context is
meaningless. There is no way to hold a resolution and conclude "therefore I may."

## What a resolution records

| Field | Why it is there |
|---|---|
| Ticket identity | What was decided, addressable later |
| The question, verbatim | A resolution read without its question is not evidence of anything |
| Options considered | The rejected alternatives are the part a summary loses |
| The option chosen, by reference | A reference, never a status word |
| Deciding human | Judgement has an author; a model may never appear here |
| Timestamp | Ordering, and replay |
| Optional bounded note | The reasoning, in the human's words |
| Superseded-resolution reference | Present only when this replaces an earlier one |

**Not recorded:** any policy, capability, ceiling, approval, or verdict; any
model-authored text presented as the decision. Model output may be *evidence
attached to* a ticket — it is untrusted data, exactly as issue text is — but the
resolution itself is authored by a human.

## Consequences

- **The frontier computation reads resolutions.** That is the intended coupling:
  a resolved ticket unblocks its dependents, deterministically, which is what
  makes the frontier derivable rather than asked-for.
- **Nothing else reads resolutions.** No tool gate, no policy check, no spawn
  authorisation, no merge. If a future slice wants a resolution to fund an
  action, that is a new decision, taken deliberately, and this ADR is the thing
  it has to argue against.
- **Test the constraint, do not assert it.** The council precedent is testable —
  a disposition cannot be turned into an approval — and the ticket resolution
  must carry the same kind of test rather than a comment saying it is safe.
- **Adding fields later is cheap; removing the meaning is not.** Everything above
  is additive. The one thing that cannot be walked back is having let a
  resolution mean "authorised" for even one release, because by then something
  will read it that way.

## Rejected alternatives

- **`status: open | approved | rejected`.** The precise failure this ADR exists
  to prevent.
- **Reusing `PlanApproval`.** It is an authorisation type. Reusing it would put
  a decision ticket one field away from granting capability.
- **Letting a model author a resolution.** Rejected. `definition-of-done.md` §4
  already fixes the pattern for model output that looks like a decision — it is
  "a **proposal the owner accepts or edits**, never a route that assigns itself".
  A model may draft the options; a human resolves.
- **Free-text resolution only.** Rejected: it loses the options considered, which
  is the part that stops a settled decision being re-argued, and it cannot be
  read by the frontier computation.
