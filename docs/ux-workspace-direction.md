# mjolnr UX direction: a governed agent workspace

**Status:** Accepted product and interaction direction; Tauri workspace and
several integrated-workspace slices landed, remaining breadth phased separately

**Written:** 2026-07-27

**Scope:** User experience, information architecture, and product boundaries. This
document does not authorize implementation or alter the current phase plan.

## Executive direction

mjolnr is evolving from a governed chat console into a **governed agent
workspace**.

The current product already has the difficult foundations: an owned agent loop,
deterministic policy gates, durable sessions, honest crash recovery, bounded
subagents, worktree isolation, evidence-backed completion, and a capable
Ratatui client. The next UX challenge is not adding more commands or decorating
the transcript. It is making those capabilities continuously understandable.

Three researched products illuminate different parts of that direction:

- **Orca** is the strongest reference for the everyday workspace: persistent
  navigation, parallel work made visible, worktrees as operational objects,
  review-oriented surfaces, and fast movement between active contexts.
- **HumanLayer / CodeLayer** is the strongest reference for mjolnr's
  approval-and-session thesis: drafts, running sessions, pending decisions,
  compressed traces, explicit operator attention, and archived history.
- **Herdr** is the strongest reference for attention management: stable
  workspace/agent hierarchy, status rollups, "done until seen," explainable
  state, and honest distinctions between live attachment, history, restore,
  and handoff.

mjolnr must implement these ideas independently. The references inform product
shape and known interaction patterns only. Their code, tests, naming, visual
identity, component structure, and internal protocols must not be copied or
ported.

The concise product position is:

> **An Orca-like rich workspace, HumanLayer-like decision flow, Herdr-like
> attention management, and a compact Ratatui companion — all rendering
> mjolnr's deterministic governance.**

The delivery surface is now decided. Ratatui remains the terminal client. The
Tauri application is the rich everyday workspace under active development; it
already consumes the shared Rust runtime and must not grow a second agent loop.
A later node canvas is additive; it does not define or precede the primary
interface. See
[`ADR 0003`](./adr/0003-shared-rust-core-tui-tauri-clients.md).

## Why the current experience has reached its limit

The current TUI is not broken. Its frame tests cover semantic rendering,
terminal sizes, approval and recovery gates, live activity, quota disclosure,
tool outcomes, command discovery, and accessibility constraints. The issue is
structural: mjolnr's capabilities have outgrown the interaction model that
contains them.

Today, most of the product is compressed into:

- one transcript;
- one composer that also acts as command launcher and, in some surfaces,
  filter input;
- twenty-three built-in slash commands plus discovered templates;
- transient overlays;
- a dense one-line telemetry header;
- contextual key behavior;
- side rails that appear only for a proposed plan or active fleet.

This was an effective way to grow the governed loop without breaking its
boundaries. It is no longer an effective way to teach or operate the whole
product.

### 1. Capabilities are present but not spatially legible

Sessions, branches, models, routes, roles, personas, skills, MCP servers,
triggers, councils, handoffs, configuration, extensions, quota, and subagent
envelopes all exist. Most are accessed by remembering a slash command, opening
an overlay, and then returning to the same transcript.

That makes mjolnr feature-rich but recall-driven. Users can discover command
names by typing `/`, but they cannot glance at the product and understand:

- what work exists;
- which work is active;
- what needs attention;
- which changes are proposed;
- what has been verified;
- what is complete but unread;
- where a decision will have an effect.

### 2. The composer carries too many meanings

The composer is the user's directive draft, slash-command query, autocomplete
anchor, and input source for some selection flows. This creates hidden modes:
the same physical area can mean "tell the agent what to do" or "control
mjolnr." Those are different intents and should have different view state.

The composer should remain the place where a user expresses intent. Selection,
navigation, configuration, search, and command discovery should have dedicated
input state.

### 3. Control state can be inferred from prose

The proposed-plan rail is currently derived from assistant text that looks like
a numbered plan. This is careful enough to avoid claiming every numbered
answer, but it still makes a control surface depend on transcript shape.

A plan that can be approved or executed should be an explicit runtime/view
state entered deliberately. Ordinary prose must remain prose. Key meanings
must not silently change because a model happened to format an answer a
particular way.

### 4. Telemetry competes with intent and attention

The header can simultaneously expose project, model, usage, policy, envelope,
estimated cost, operation budget, tool-call budget, next-context estimate,
quota, and full-auto counts. All are valid facts, but not all deserve permanent
visual priority.

The primary frame should answer:

1. Where am I?
2. What is mjolnr doing?
3. What needs my decision?
4. What is the governing policy?

Cost details, quota windows, routing diagnostics, and extended budgets should
remain available in contextual detail surfaces.

### 5. Setup and authentication feel like another product

Guided onboarding and credential capture correctly preserve secret boundaries,
but the experience moves between the polished mission-control TUI and
plain-terminal flows. The security boundary should remain; the interaction
continuity should improve.

mjolnr should frame setup as one guided journey, even when a particular secret
must be captured outside the transcript. Returning from credential capture
should land in an explicit result step that says what changed, what was
verified, and what remains unavailable.

### 6. The trust story is vulnerable to small UI/runtime mismatches

The current composer attachment caption still says an image is attached as a
path and the model receives text, while Phase 29 now sends bounded image bytes
to supported providers and refuses unsupported paths explicitly.

This is a small copy defect with large product significance. mjolnr's promise is
that the UI never lies about runtime state. UX work must therefore include a
systematic pass over every user-facing capability claim, not only layout and
styling.

## What must be preserved

The redesign must not weaken the parts mjolnr already does unusually well.

### Deterministic authority

Every status displayed as authoritative must be derived from mjolnr-owned
runtime, policy, persistence, or tool events. Model prose can be shown as a
proposal, but it cannot create execution authority or claim a verified state.

### Approval and recovery remain different interactions

"May mjolnr do this?" and "mjolnr was interrupted and cannot know whether this
happened" are not the same decision. They must retain different language,
controls, and event records.

### Exact effects stay visible

An approval must show the exact diff, argv, destination, or other bounded
effect. A workspace redesign must not reduce this to a generic "allow agent"
button.

### Append-only durable truth

Conversation, decisions, tool outcomes, recovery facts, and verification
evidence remain projections of the durable record. Navigation and summary
surfaces may compress it, but they must not rewrite history.

### Terminal operation remains first-class

mjolnr remains terminal-launchable and Ratatui remains supported. A wide
terminal may show a small number of useful regions; a narrow terminal should
switch between complete views rather than squeeze an imitation desktop UI into
unreadable columns. “First-class” does not mean “the only rich surface.”

### Every interface stays a client

The runtime owns state. Ratatui and Tauri render snapshots and emit semantic
commands. No visual surface may become a second execution path.

## Inspiration from Orca

Orca is best understood as a desktop orchestration workspace for running many
CLI agents in isolated worktrees. mjolnr should not adopt that underlying
product thesis, but several of Orca's interaction choices are highly relevant.

### Adopt: persistent operational objects

Orca treats a worktree as more than a folder. It is an operational card with
identity, status, agent activity, issue or pull-request context, ports, runtime
state, and available actions.

mjolnr needs an analogous first-class **work item**. Depending on context, that
item may represent:

- a draft directive;
- an active session;
- a resumed session;
- a branch or fork;
- a subagent or council member;
- work awaiting approval;
- work awaiting review;
- a completed or failed run.

The user should not have to reconstruct that identity from transcript messages.

### Adopt: a universal jump surface

Orca's jump palette spans recent worktrees, open tabs, projects, settings, and
actions. mjolnr should provide one keyboard-first jump surface across:

- active and recent sessions;
- drafts;
- branches and forks;
- agents requiring attention;
- pending approvals and recovery decisions;
- proposed changes;
- available actions and settings;
- files mentioned by the active work;
- commands and prompt templates.

Slash commands can remain expert shortcuts, but the jump surface becomes the
primary navigation and discovery mechanism.

### Adopt: changes as a primary surface

Reviewing an agent's work is not the same activity as reading its conversation.
mjolnr should give proposed and completed changes their own surface with:

- changed-file list;
- exact unified diff;
- proposal versus applied state;
- relevant tool and approval events;
- verification attached to the affected change;
- human notes or requested corrections;
- an explicit route back to the transcript context.

This does not make the TUI an editor. It makes review a first-class governed
activity.

### Adopt: explicit viewport intent

Live output should not drag a user away from text they are reading. mjolnr should
model "following newest output" and "reading a pinned viewport" explicitly,
rather than inferring user intent indirectly.

### Adapt: worktree visibility

mjolnr already isolates subagents in worktrees. Their existence should become
visible through the work hierarchy: branch, parent task, policy ceiling,
budget, live activity, attention state, and result. Worktree management remains
owned by the runtime and tools, not by a general-purpose terminal pane.

### Adapt: the surrounding developer workspace

The first workspace deliberately stopped before the surrounding developer
environment. File editing, terminal management, repository controls, and task
integration have since landed in bounded slices. Embedded browsing, remote
work, and external CLI-agent adapters remain product direction, only behind the
trust and authority boundary in
[`ADR 0006`](./adr/0006-bounded-integrated-developer-workspace.md).

mjolnr's integrated workspace covers or is expected to cover:

- a file explorer and code editor;
- terminal tabs and splits;
 - GitHub and Linear task/PR integrations, plus Vercel deployments and Supabase projects as bounded read-only `TaskSource`s (same `HashMap<IntegrationId, Arc<dyn TaskSource>>` registry, `Secret`-redacted, `TaskId` charset; no PR destination — `submit_change` refuses typed `Unavailable`);
- a per-worktree browser with bounded design inspection;
- SSH-backed remote workspaces;
- explicit provider and integration account profiles; and
- a compatibility surface for external CLI agents.

This is adaptation, not wholesale adoption. mjolnr work items remain the
organizing identity. GitHub and Linear supply or receive work; they do not own
it. Terminals and direct editor actions are operator-controlled. An arbitrary
external CLI agent is external-unverified unless a specific adapter proves that
every one of its side effects is forced through mjolnr's policy and tool proxy.
The interface must not blur those trust classes.

### Still reject from Orca

mjolnr should reject an unbounded IDE roadmap assembled merely by matching
Orca's feature list, PTY-mediated "native chat" presented as mjolnr-governed,
screen-derived authority, silent credential inheritance, and any browser,
terminal, editor, Git, task, or SSH action implemented directly in the
frontend. mjolnr owns its native agent loop and must not simulate governance by
typing into another product.

## Inspiration from HumanLayer / CodeLayer

The researched HumanLayer repository describes both an older approval SDK and
a CodeLayer desktop/session experience. Its root README says the checked-out
code is deprecated and superseded, so it is evidence of interaction ideas, not
a guaranteed representation of the current commercial product.

### Adopt: explicit work lifecycle

mjolnr should distinguish:

```text
Draft → Active → Needs decision → Reviewing → Verified / Failed → Archived
```

These states reduce cognitive load because preparing work, supervising work,
deciding risk, reviewing effects, and reading history are no longer mixed into
one list.

The names are illustrative; final reason codes and lifecycle types must be
derived from mjolnr's runtime contract.

### Adopt: approval-first navigation

A governed agent workspace needs a global answer to:

> What is the next risky thing waiting for me?

Pending approval, uncertain recovery, quota stop, failed verification, and
completed-but-unreviewed work should roll up into an attention queue. From any
view, one action should move to the highest-priority unresolved item.

The queue is a projection over durable events, not a separate mutable inbox.

### Adopt: off-screen attention remains visible

If an approval or failure is outside the current transcript viewport, mjolnr
must retain a visible, selectable indicator. The user should never need to
scroll randomly to discover why work stopped.

### Adopt: trace compression

Long tool and subagent traces should render as semantic groups:

- task or subagent name;
- current state;
- elapsed time and bounded budget;
- latest meaningful action;
- approvals or failures;
- stable outcome;
- expandable raw detail.

Groups should expand automatically only when attention is required. Stable
outcomes must remain visible even when details are collapsed.

### Adopt: compact progress projection

HumanLayer's TODO sidebar demonstrates a useful pattern: derive a small progress
view from the latest structured task state instead of forcing the user to
search the transcript.

mjolnr can generalize this into an explicit Plan surface, derived only from
structured plan state. It must remain labelled as proposed until the runtime
has accepted it, and it never grants authority by itself.

### Adopt: quick launch and durable drafts

Starting work should require only:

- directive;
- workspace;
- provider/model or route;
- policy;
- optional budget/persona.

Drafts should survive navigation and restart without becoming active sessions
or triggering provider calls. Creating a draft is not permission to act.

### Adopt: visible reversibility

Safe organizational actions such as archive, rename, or move between views
should be reversible and should say so. Execution effects are governed
separately and must not be disguised as ordinary undoable UI state.

### Reject from HumanLayer

mjolnr must not add a lightweight "dangerously skip permissions" affordance.
Full-auto remains an explicit policy with unmistakable confirmation, bounded
authority, recorded automatic approvals, and no persistence across resume.

mjolnr should also avoid making a daemon-plus-desktop-shell architecture its
default identity. A future remote client may exist, but it should consume the
same runtime truth rather than redefining the product around an external agent.

## Inspiration from Herdr

Herdr is an agent-aware persistent terminal workspace. mjolnr should not become
an external-agent multiplexer, but Herdr shows how to make concurrent work
glanceable.

### Adopt: a small, stable hierarchy

Herdr's workspace → tab → pane → agent hierarchy keeps its UI coherent.
mjolnr's corresponding hierarchy should be small and product-specific:

```text
Workspace
└── Work item / session
    ├── branch or fork
    ├── agent run
    ├── proposed/applied changes
    ├── decisions
    └── verification evidence
```

The exact storage schema need not mirror the visual tree. The hierarchy is a
user mental model and a projection over existing durable identities.

### Adopt: status rolls upward

If a child agent is blocked, its session and workspace should show that
attention is required. If several children are running, the parent should show
an aggregate without hiding individual states.

Rollups should use a restrained vocabulary grounded in mjolnr events:

- draft;
- queued;
- thinking/responding;
- proposing;
- waiting for approval;
- executing;
- verifying;
- uncertain;
- refused/failed;
- verified;
- complete but unread.

### Adopt: done until seen

Idle and newly completed are different operator states. A completed agent
should remain marked as new or unread until the user visits its result. Merely
having no work in flight must not erase the attention signal.

### Adopt: explainable status

Every important state should answer "why?" The detail may show:

- event or reason code;
- governing policy and authority source;
- exact approval or refusal;
- latest runtime activity;
- evidence supporting verification;
- what remains unknown.

This is especially valuable for model-governance ceilings, extension loads,
spawn envelopes, and recovery decisions.

### Adopt: honest continuation vocabulary

Live attachment, full resume, compact resume, new session from handoff, branch,
fork, clone, and archived history are different operations. The UI should name
them consistently instead of collapsing them into a generic "continue."

### Reject from Herdr

mjolnr must not infer execution-critical state from terminal screen patterns.
It must also reject unrestricted plugins that run with the user's full
authority. Extensions remain declarative, capability-scoped, session-loaded,
and governed by the ordinary execution gate.

## Target information architecture

The hierarchy and surfaces below are shared product concepts, not a requirement
that every client display every concept simultaneously. Tauri is the intended
full expression. Ratatui should expose the same semantic state through focused
views, contextual overlays, and shortcuts.

### Tauri workspace composition

The rich client should resemble Orca more than a desktop-sized TUI: persistent
work navigation, a clear selected work object, dedicated conversation/plan/
changes/verify views, and contextual attention. Onboarding belongs in this
application rather than as an afterthought outside the main interface.

The node canvas may later become another primary surface backed by the same
runtime work objects. It is not the shell and should not dominate initial
desktop architecture.

### Wide-terminal companion composition

```text
┌ WORK ──────────┬ CONVERSATION  PLAN  CHANGES  VERIFY ┬ ATTENTION ───────┐
│ Active         │                                      │ Approval needed │
│ Needs review 2 │       Current task surface           │ Exact effect    │
│ Drafts         │                                      │ Evidence/risk   │
│ Archive        │                                      │ Approve / deny  │
├────────────────┴──────────────────────────────────────┴─────────────────┤
│ Directive…                                    model · policy · status   │
└─────────────────────────────────────────────────────────────────────────┘
```

This is a compact terminal relationship model, not the Tauri pixel
specification.

- **Work rail:** persistent navigation and attention rollups.
- **Primary surface:** one selected task-oriented view.
- **Attention/detail rail:** conditional; appears when a decision or focused
  detail warrants it.
- **Composer:** user intent only.
- **Context line:** the smallest truthful set of live state.

### Narrow-terminal composition

Narrow terminals should show one complete surface at a time:

```text
WORK | CONVERSATION | PLAN | CHANGES | VERIFY | ATTENTION
```

A stable focus/jump action changes surfaces. Pending approval or recovery may
still take over the frame because blocking authority outranks navigation.

### Primary surfaces

#### Work

- Active sessions and drafts.
- Needs-attention group.
- Completed-but-unread group.
- Archive/history.
- Branch/fork and parent-child relationships.
- Compact model, policy, activity, and budget status.

#### Conversation

- Durable user/model narrative.
- Proposed actions and stable tool outcomes.
- Collapsed structured activity groups.
- Explicit distinction between proposal, action, and verified result.

#### Plan

- Structured proposed steps.
- Active/completed step projection.
- Plan source and status.
- Approval, revision, or exit actions only when the runtime is explicitly in
  a plan-review state.

#### Changes

- Changed files.
- Proposed versus applied diff.
- Read-before-edit evidence.
- Approval event.
- Human feedback.
- Link to originating transcript/tool activity.

#### Verify

- Commands and checks executed.
- Exit status and bounded output.
- Claims each check supports.
- Known gaps.
- Final result: verified, failed, refused, uncertain, or incomplete.

#### Attention

Priority order:

1. durability lost;
2. uncertain recovery;
3. approval required;
4. verification failed;
5. quota/budget stop;
6. completed but unread;
7. informational notice.

This preserves the current blocking-overlay precedence while extending it
across multiple work items.

## Core journeys

### First launch

1. mjolnr identifies the workspace.
2. Guided setup shows progress and explains which artifacts will be written.
3. Provider authentication retains its secret boundary.
4. Returning to mjolnr shows the verified provider/model state.
5. The quick launcher opens with a ready default route and ask policy.

### Start work

1. User writes or resumes a durable draft.
2. User selects route/model, policy, and optional budget.
3. mjolnr previews the effective authority.
4. Starting creates an active session and makes no broader grant.

### Supervise work

1. Work rail shows live activity and parent/child relationships.
2. Conversation shows narrative and bounded outcomes.
3. Plan shows explicit structured progress.
4. The user may continue reading without live output stealing the viewport.

### Decide a side effect

1. Attention rail and work rollup signal the pending decision.
2. Decision view shows intent, exact effect, policy tier, and relevant context.
3. User approves once, approves an exact command for the session where allowed,
   or denies.
4. The durable record captures the source and scope of the decision.

### Review completion

1. Completed work remains unread until visited.
2. Changes surface shows what changed.
3. Verify surface shows evidence and honest gaps.
4. User may archive, request follow-up, fork, or hand off.

### Recover interrupted work

1. Recovery interrupts ordinary navigation.
2. mjolnr distinguishes proven-not-started from uncertain effect.
3. No automatic retry occurs.
4. User chooses the recovery action using controls distinct from approval.

## Interaction rules

1. The composer expresses intent; it is not shared picker state.
2. One universal jump surface handles navigation, actions, and discovery.
3. Slash commands remain shortcuts into the same semantic commands.
4. Key behavior is derived from explicit focus and mode, never prose shape.
5. Pending authority is visible even when its transcript event is off-screen.
6. Live output never steals a deliberately pinned viewport.
7. Important states expose their reason and authority source.
8. Stable outcomes remain visible when raw detail is collapsed.
9. Colour reinforces state but never carries it alone.
10. No UI surface may imply an effect occurred before the durable runtime says
    it did.

## Open-source and commercial optionality

mjolnr is licensed under Apache-2.0 and remains unpublished with `publish = false`.
The repository is being prepared for a later public release; publication and
registry distribution remain separate owner decisions.

The essential local experience should remain usable from the open-source
repository without a hosted entitlement or monthly subscription:

- local agent harness;
- policy and approval engine;
- TUI;
- durable store and recovery;
- provider adapters;
- worktree/subagent isolation;
- extension and skill contracts;
- local changes and verification review.

This boundary supports trust: users can inspect and own the code that enforces
the rules. It also avoids coupling product-market learning to a premature
billing decision.

Optional commercial services could remain outside the local core:

- cross-device approval delivery;
- shared team queues and collaboration;
- hosted schedules or always-on workers;
- organization policy distribution;
- long-retention audit and compliance exports;
- managed provider routing or relay;
- fleet dashboards;
- enterprise identity and administration;
- remote access and synchronized clients.

This permits a public local product alongside optional paid hosting or team
services later. Essential local approvals and evidence do not become monthly
entitlements merely because remote services may be commercial.

## Adopt, adapt, reject

| Reference | Adopt | Adapt | Reject |
|---|---|---|---|
| Orca | Persistent work objects, universal jump, changes/review surface, viewport intent | Worktree/agent hierarchy, editor, terminals, bounded browser/design mode, SSH, task sources, and external CLI compatibility through explicit trust classes | Unbounded feature copying, screen-derived authority, PTY shims presented as governed mjolnr sessions |
| HumanLayer | Draft/active/attention/archive lifecycle, approval-first navigation, trace compression, quick launch | Desktop modals into responsive Ratatui surfaces; drafts into mjolnr durability | Casual permission bypass, desktop daemon as the product identity |
| Herdr | Stable hierarchy, upward status rollups, done-until-seen, explainable status | Terminal/agent status into explicit trust classes and deterministic mjolnr events | Screen-pattern authority, unrestricted plugins, external multiplexer state presented as mjolnr authority |

## Delivery direction

The earlier Ratatui-only sequence below is retained as design history. It is no
longer authorization to continue adding panels. The approved replacement is
[`docs/tauri-path-and-phases.md`](./tauri-path-and-phases.md), supported by the
desktop contract in
[`docs/tauri-design-system.md`](./tauri-design-system.md).

Phases A0–C have since landed. The approved next-work sequence is
[`docs/integrated-workspace-phases.md`](./integrated-workspace-phases.md):
runtime contracts and trust classes first, then hierarchy, child work, exact
review, deterministic search, Git/task integrations, editor, terminal,
external-agent compatibility, bounded browser/design mode, SSH/account
profiles, and a daily-driver release checkpoint.

The expansion keeps the selected shadcn-svelte Nova/Zinc component system,
Cyan charts, HugeIcons, Geist Mono, small radius, default/translucent menus,
and system-following light/dark themes. The roadmap changes information
architecture and capability, not the agreed visual foundation.

### Approved gate before rich desktop workflow

- Make plans, interview questions, review verdicts, and human approval
  runtime-owned facts rather than transcript/TUI inference.
- Decide the minimum TUI repair needed to preserve a capable terminal client.
- Prove both clients can consume semantic commands and snapshots without
  duplicating governance.
- Start Tauri once that boundary is sufficient, not once every conceivable TUI
  surface is complete.
- Defer graph expansion and the node canvas unless evidence makes either a
  prerequisite.

The remaining UX 0–4 text describes useful capabilities, but its former
Ratatui-only delivery assumption is superseded by ADR 0003.

### UX 0 — Contract and prototypes

- Define the work-item and attention-state vocabulary.
- Map every current command and overlay into the target surfaces.
- Produce wide, medium, and narrow terminal wireframes.
- Identify which states already exist in runtime snapshots and which require
  new read-only projections.
- Audit user-facing copy against actual runtime behavior, including image input.
- Set frame-test acceptance before changing layout.

**Stop condition:** reviewed interaction contract with no runtime changes.

### UX 1 — Navigation shell

- Add work rail and narrow-terminal surface switcher.
- Add universal jump/action palette with dedicated query state.
- Keep the existing transcript and gates intact inside the new shell.
- Reduce permanent header telemetry to project, model/route, policy, live state,
  and pending attention.

**Stop condition:** existing governed loop remains behaviorally unchanged and
all current safety frame tests still pass.

### UX 2 — Work lifecycle and attention

- Surface drafts, active work, needs-attention, unread completion, and archive.
- Add deterministic rollups for subagents and council members.
- Add global next-attention navigation.
- Add explicit follow-output versus pinned-viewport state.

**Stop condition:** every attention state is derived from durable/runtime truth
and has negative tests against false claims.

### UX 3 — Plan, changes, and verification

- Replace transcript-heuristic plan authority with explicit structured state.
- Add changes review surface.
- Add verification/evidence surface.
- Add semantic inspectors and grouped tool/subagent traces.

**Stop condition:** proposal, applied effect, and verified outcome remain
visually and mechanically distinct.

### UX 4 — Onboarding continuity

- Unify setup progress and result framing.
- Preserve secret capture boundaries.
- Land first-time users in a ready quick launcher.
- Test first launch, partial provider failure, re-authentication, and return to
  an existing session.

**Stop condition:** setup feels like one mjolnr journey without placing secrets
in the transcript, argv, logs, or view state.

## Success criteria

The redesign succeeds when a new or returning user can answer, without
remembering a command:

- What work exists?
- What is running?
- What needs me?
- What changed?
- What did mjolnr verify?
- What remains uncertain?
- Which model and policy govern this work?
- Is this action mjolnr-governed, operator-controlled, or external-unverified?
- How do I resume, branch, hand off, or archive it?
- How do I inspect, edit, run, review, verify, commit, and publish the selected
  work without losing its provenance?

It must also preserve:

- zero side effects outside deterministic gates;
- no model-created authority;
- no automatic retry of uncertain effects;
- append-only durable truth;
- responsive terminal operation and a capable terminal client;
- one authoritative Rust runtime across Ratatui and Tauri;
- current accessibility and semantic-colour guarantees;
- independently implemented provenance.

## Evidence reviewed

### mjolnr

- `docs/tui-design.md`
- `src/tui/layout.rs`
- `src/tui/reducer.rs`
- `src/tui/commands.rs`
- `src/tui/chrome.rs`
- `src/tui/keymap.rs`
- `src/tui/app.rs`
- `src/cli/onboard.rs`
- `tests/tui_frames.rs`
- `README.md`
- ### Orca

- `README.md`
- `docs/cmd-j-tab-session-search.md`
- `docs/terminal-main-owned-state.md`
- `docs/terminal-scroll-intent-architecture.md`
- `docs/native-chat-codex-tui-parity.md`
- renderer application shell, sidebar, worktree card/list, task page, quick
  open, and jump-palette sources
- orchestration and CLI skill guides

### HumanLayer / CodeLayer

- root `README.md` and `humanlayer.md`
- daemon protocol, session, approval, and event-stream sources
- WUI router, session table/detail, launcher, approval navigation, grouping,
  TODO, hotkey, and architecture sources

### Herdr

- root `README.md`
- concepts, agents, automation, session-state, keyboard, and plugin docs
- workspace/tab/pane/agent API schemas
- client input, persistence restore, and multi-client tests

Reference checkouts used during product research are not part of this
repository. They are research inputs, not dependencies or implementation
sources.
