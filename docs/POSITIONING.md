# smed — a harness that helps builders ship

smed is a coding harness you run in your project. It reads, edits, runs commands,
and verifies, in the directory where you launched it, through model accounts you
control. The point is that work happens — and that you can see every step of it
before it happens and after it lands. `smed` is Danish for "smith": a short name
for a tool concerned with the quality of the work and the evidence left in your
hands.

## Build, coordinate, trust

- **Build.** Read, write, and run in your repo. Reads are bounded and
  evidence-first, writes are read-before-edit, commands are exact argv, and
  completion is evidence-gated. Your lint, tests, and CI stay yours.
- **Coordinate.** Run it as a terminal console, headless in scripts and CI, or on
  scheduled and webhook triggers. Spawn subagents in isolated worktrees, route to
  the right model per role, and review a council of responses.
- **Trust.** The loop is built so a refusal is structural rather than requested.
  That is construction quality, not a category — it is how smed is *built*, not
  what smed *is*.

## A native harness with room to connect

smed owns a capable native loop and talks directly to model providers. That is
the dependable path, not a claim that every useful capability must be rebuilt
inside one process. External agents, richer recall, and stronger execution
environments can join the workspace when they carry explicit provenance and do
not bypass the same authority boundaries.

The distinction is visible rather than rhetorical. Native governed work,
operator-controlled actions, and externally produced work are different trust
classes. A future search or memory projection may help find context, but it does
not rewrite the ledger. A future sandbox may contain a process, but the product
does not claim that containment before it exists. Expansion is welcome; blurred
state is not.

## How it is built

The difference from a rules file is that the rules are code. Advice arrives as
*content* into a loop somebody else owns: prose guardrails are text the model
weighs, a confidence score is a suggestion by construction, and hook callbacks run
at the host's discretion and can be shed with the config that declared them. Each
of those can be written more carefully; none can be made binding from outside the
loop. smed owns its loop, so its rules are the structure of the loop itself.

- **A total, fail-closed policy gate.** Every tool call passes through one pure
  function of (policy mode, tool tier, session approval). There is no code path
  around it, no error state that falls open, no config file the agent can talk its
  way past.
- **Evidence-gated completion.** The agent cannot declare work "verified" by
  asserting it. It must cite a successful command, recorded in the ledger, that ran
  *after* the last mutation — or the claim is refused with a typed reason code.
- **An append-only audit ledger.** Every proposal, approval, result, and refusal is
  an event in SQLite with a database-enforced sequence. The record is not a log of
  the run; the record *is* the run — sessions replay and recover from it. If an
  event cannot be persisted, the tool does not execute.
- **Clamped delegation.** A child never gets authority a *human* did not grant.
  Read-only is absorbing: a read-only parent delegates nothing wider, and a child
  asked to be read-only stays that way under any parent. Full-auto is never
  inherited — it takes a full-auto parent *and* an explicit request. An `ask`
  parent can delegate autonomous writes only through the gate everything else
  passes: each spawn is previewed with every child's policy, budget, and
  directive, and approved individually — the approve-this-exact-command shortcut
  does not reach spawns. Enforced in the spawn path, not requested in a prompt.
- **Merge is a separate act.** A child's work is committed to its own branch in
  its own worktree and stops there. The parent gets a schema-validated result and
  a branch reference; nothing lands because an agent decided it was finished.
- **Width comes from a better authorisation, not a bigger number.** Raising a
  fan-out constant eventually outgrows what anyone reads, and the approval becomes
  a reflex. smed's cap is four children *without an envelope* — what one human can
  read in one preview — and an envelope raises it by changing what is approved: a
  human authorises a bounded *shape* once, the runtime enforces it per spawn, and
  every draw is recorded against the grant that permitted it. A draw that does not
  fit is refused with a typed code rather than downgraded to a prompt nobody would
  read.

Governance floors — refusing a fixed set of dangerous actions outright — are
becoming common, and they are a real improvement. Floors bound the worst case.
smed disposes of every case instead, by intent and by evidence.

The interface promise is complementary: operate that governed work through a fast
terminal client or a desktop workspace without changing which runtime owns
authority.

## Why not a plugin?

The incumbent harnesses expose hook callbacks, and most of smed's gate *could* ride
them — we checked, contract by contract. What can't: hooks fail open on error, stop
themselves after a bounded number of blocks, can't clamp a subagent's authority, and
live in user config the governed session can shed. Through that seam, deterministic
enforcement degrades into determined advice. The guarantee only exists if the loop
itself provides it — so smed *is* the loop, and other systems attach to smed: tools
come in over MCP (gated like everything else), runs are triggered over webhooks, and
approval surfaces render wherever the human is.

## What the guarantee does not cover

Worth stating plainly rather than in a footnote, because the claim is easy to
over-read. Evidence-gated completion proves that a command ran green after the last
mutation. It does not prove the change was *right*. A passing build around a subtly
wrong edit satisfies every gate described above — and that failure mode is the one
that does the most damage in practice.

smed governs two things: whether an action is permitted, and whether a completion
claim is demonstrable. Correctness is not among them, and a governed harness that
implied otherwise would be selling the same false confidence it exists to remove.

## The pitch in one line

smed does the work with you — reads, edits, runs, verifies — and is built so every
step is permissioned, recorded, and evidenced. Governance is how it is built, not
what it shouts about.
