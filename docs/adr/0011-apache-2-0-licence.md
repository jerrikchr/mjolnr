# ADR-0011: Apache-2.0 as the project licence

**Status:** Accepted
**Date:** 2026-07-31
**Deciders:** Jerrik Christiansen  
**Phase:** Pre-release positioning

## Context

The licence was deliberately left unselected from the first commit. `AGENTS.md`
§8 forbade inventing an identifier, both manifests carried `publish = false`, and
`deny.toml` restricted the dependency graph to permissive licences specifically so
that no transitive dependency could pre-empt the choice. `THIRD_PARTY.md` records
one occasion where that constraint bit: `option-ext` (MPL-2.0) was rejected rather
than the allowlist widened, on the grounds that widening it would have made the
owner's decision for him inside an unrelated phase.

That posture was correct while the decision was open, but it had become the single
blocking item before any public release. Every other pre-release task — README
framing, positioning, provenance — could be done under any licence. None of them
could ship without one.

## Decision

**smed is licensed under Apache License 2.0.**

- `LICENSE` holds the canonical text from `apache.org/licenses/LICENSE-2.0.txt`,
  verbatim and unmodified, including the appendix. It is never to be edited.
- `NOTICE` holds the copyright line and points at `THIRD_PARTY.md`.
- Both `Cargo.toml` and `desktop/src-tauri/Cargo.toml` declare
  `license = "Apache-2.0"`.
- `deny.toml` no longer sets `private = { ignore = true }`. Our own package is now
  checked against the allowlist like any other crate in the graph.

## Why Apache-2.0 over MIT

MIT is the shorter licence and the more common default in the Rust ecosystem, and
for most libraries the difference is immaterial. It is not immaterial here.

**The patent grant is the reason.** Apache-2.0 §3 grants an explicit, irrevocable
patent licence from contributors to users, and terminates it for anyone who brings
a patent suit over the work. MIT grants copyright permissions and says nothing
about patents. smed's entire pitch is a set of *enforcement mechanisms* — a
fail-closed gate, evidence-gated completion, clamped delegation, spawn envelopes.
Mechanisms are the category of thing patents cover; prose and APIs mostly are not.
A licence that leaves patent rights implicit is a worse fit for this project than
it would be for a serialisation crate, and the cost of the better fit is a longer
file nobody reads either way.

Secondary, and genuinely secondary: Apache-2.0 §4 requires modifiers to state
their changes, which sits well with a project whose stated ethos is never lying
about state. It is a weak requirement and was not decisive.

## Alternatives rejected

**Dual MIT OR Apache-2.0.** The Rust ecosystem convention, and the recommendation
carried in  *downstream crates* pick the licence that matches their own, which matters
for a library that gets linked into other people's dependency trees. smed is an
application. Nobody links it. Offering a choice that has no consumer buys ambiguity
about which terms apply, and gives up the patent grant to anyone who selects the
MIT arm — which is to say, it gives up the entire reason for choosing Apache-2.0.

**MIT alone.** See above. Shorter, more familiar, no patent grant.

**A copyleft licence (GPL/AGPL/MPL).** Rejected on positioning grounds rather than
philosophical ones. smed's strategic bet, recorded in `docs/POSITIONING.md`, is
that other systems attach to it: tools arrive over MCP, runs are triggered over
webhooks, approval surfaces render wherever the human is. That is an interop bet,
and copyleft narrows exactly the population able to take it. Adoption is the
positioning; a licence that caps adoption caps the positioning.

**Business-source or source-available.** Not seriously considered. The project was
not built to earn revenue, and a licence that forecloses commercial use would cost
the credibility the open record is meant to buy while protecting a business that
does not exist.

## Consequences

- The permissive-only rule in `deny.toml` **survives the decision on stronger
  grounds.** Before, a copyleft dependency would have narrowed a pending choice.
  Now it would force a relicence of shipped code. The rule stays, and the
  `option-ext` rejection stays correct.
- `cargo deny check` will now fail if the declared licence is ever changed to
  something outside the allowlist, which is a mechanical guard on this ADR.
- **`publish = false` stays.** Publishing to crates.io is a separate decision about
  release readiness and name availability, and it has not been made. `AGENTS.md` §8
  now tracks the two independently; previously they were one bullet, which is why
  they were confused for one decision.
- Apache-2.0 §4(d) obliges downstream redistributors to carry `NOTICE` forward.
  This is a real, if small, burden placed on others by choosing to have a NOTICE
  file at all. Accepted: the copyright line has to live somewhere, and the
  alternative is an unattributed `LICENSE` with the appendix's `[yyyy] [name of
  copyright owner]` placeholders left unfilled.
- Per-file licence headers are **not** adopted. Apache-2.0's appendix recommends
  them; the Rust ecosystem overwhelmingly does not use them, and 93k lines of
  header comments would degrade the source for no legal gain that `LICENSE` and
  the manifest declarations do not already provide.

## Not decided here

- Whether to publish to crates.io, and under what package name.
- Whether ships in the public repository. The owner's stated
  intent is to withhold it; that is a content decision, not a licensing one.
- A contributor licence agreement or DCO. Not needed while the project has one
  author; revisit if outside contributions arrive.
