# mjolnr integrated developer workspace phases

Status: **D0–D8 have landed in bounded slices. Repository, task integration,
editor, and terminal capabilities exist, with documented breadth and live-
journey gaps. D9–D12 remain open.**

| Phase | State |
|---|---|
| D0 | **Landed.** Trust classes and authority boundaries are part of the client contract. |
| D1 | **Landed.** Persistent work hierarchy and attention rollups are runtime projections. |
| D2 | **Landed.** Worktree and child-run controls reuse the native subagent runtime. |
| D3 | **Closed.** `ClientSnapshot.changes` carries exact working-tree diffs; read evidence cites durable tool results; revision-anchored review threads survive restart and refuse stale relocation. |
| D4 | **Closed.** Deterministic workspace search is maintained with the durable record and exposed through the command palette. Filter, pagination, and index breadth remain surface work. |
| D5 | **Landed with explicit limits.** Repository state, path-level controls, and governed `fetch`/`push` exist. Hunk staging remains refused. Remote synchronization is qualified by the last-seen tracking reference. |
| D6 | **Landed.** GitHub, Linear, Vercel (6.1) and Supabase (6.2) sources perform bounded network requests; imported text stays untrusted, remote revisions pin write-back, and the desktop Board exposes task, pull-request, and review actions. |
| D7 | **Landed with manual breadth remaining.** Contained file reads and stale-safe saves feed the desktop explorer, CodeMirror editor, tabs, go-to-file, local editor controls, and Rust-owned context diagnostics. Packaged mutation and assistive-technology journeys remain manual gates. |
| D8 | **Automated multiplexer surface and packaged shell validation landed.** Rust owns bounded scrollback/search, cwd containment, process-group stop, tabs, horizontal/vertical split metadata, restart, and explicit close; a real packaged PTY journey after workspace selection remains open. |
| D9–D12 | Open and dependency-gated. D9 depends on the complete D8 multiplexer; D10/D11 depend on D0; D12 depends on D1–D11. |

“Landed” does not mean every live journey is release-proven. Each row names its
remaining limit. D3 and D4 are marked closed because their acceptance boundaries
have producers and negative tests; D5–D8 retain explicit capability or manual-
validation gaps.

## Recommended execution order

The D-phase numbers are stable identifiers, not a priority queue — they were
allocated by dependency order at planning time, not by what's cheapest or
riskiest to build next. They are not renumbered here: the letters are load-
bearing across ADRs, reports, and branch names, and a renumbering would
cost more in broken cross-references than it returns in a tidier sequence.
This section instead states the actual recommended build order, separate from
the D-number:

1. Close the remaining packaged editor and terminal journeys without widening
   their authority boundaries.
2. Implement D9 external-agent compatibility on top of D8, preserving
   `ExternalUnverified` provenance through review and commit.
3. Build D10 browser/design inspection and D11 remote/account surfaces as
   independent, separately verified slices.
4. Run D12 only after D1–D11 can be exercised as one packaged daily-driver
   journey.

Date: 2026-07-29
Status updated: 2026-08-17
Predecessor: [`tauri-path-and-phases.md`](./tauri-path-and-phases.md)
Architecture: [`ADR 0006`](./adr/0006-bounded-integrated-developer-workspace.md)
Product direction: [`ux-workspace-direction.md`](./ux-workspace-direction.md)
Engineering contract: [`AGENTS.md`](../AGENTS.md)

## 1. Requirements summary and outcome

Turn the Phase C desktop client into a daily-driver developer workspace without
creating a TypeScript agent engine or weakening mjolnr’s governed-execution
claim.

The completed A0–C work provides an in-process Tauri bridge, runtime-owned plan
authority, a cross-client planning slice, and the initial Conversation, Plan,
Changes, Verify, and Attention surfaces. Later slices added much of the
surrounding development loop while preserving the same ownership boundary:

- exact file changes and line-level review;
- visible worktrees, subagents, and parent/child runs;
- deterministic session/activity search;
- governed local Git and pull-request workflows;
 - GitHub, Linear, Vercel and Supabase task sources;
- file exploration and editing;
- terminal tabs and splits;
- external CLI-agent compatibility;
- browser-based design inspection;
- SSH workspaces and account selection.

Out of scope for D0–D12: a node canvas, hosted execution, team collaboration,
mobile companion, learned/semantic recall, and a daemon/RPC boundary. Those
require separate evidence and decisions.

## 2. Current baseline and gaps

- `ClientSnapshot` currently exposes messages, sessions, approvals, recovery,
  models, and plan state, but no worktree tree, change set, review-note,
  repository, terminal, browser, integration, or SSH DTO
  (`src/core/client/types.rs:20-41`).
- `ClientCommand` currently covers project/session/plan/approval/recovery
  intents, but no Git, review, task, editor, terminal, browser, or remote
  operations (`src/core/client/command.rs:9-92`).
- Subagents already use isolated Git worktrees and never merge implicitly
  (`src/runtime/subagent/worktree.rs:1-12`).
- The desktop Changes surface explicitly discloses that it lacks file-level
  change and diff DTOs
  (`desktop/src/lib/components/surfaces/ChangesSurface.svelte:31-74`).
- The desktop shell already provides the five primary workspace surfaces,
  command palette, sidebar, and inspector entry points
  (`desktop/src/routes/+page.svelte:49-72`, `162-203`).
- Session querying is deterministic and bounded but scoped to one active
  session; it is not yet a cross-session search index
  (`src/runtime/session_query.rs:1-18`, `84-109`).

## 3. Principles

1. **Authority before controls.** A visible action is not enabled until a typed
   Rust command, refusal path, durable intent, and verified outcome exist.
2. **Trust class is always visible.** mjolnr-governed, operator-controlled, and
   external-unverified work must never share an ambiguous status vocabulary.
3. **Work is the organizing object.** GitHub, Linear, terminals, files,
   browsers, and external agents attach to mjolnr work items.
4. **Read first, mutate later.** Each integration lands as a bounded read-only
   projection before its modifying commands.
5. **One phase, one checkpoint.** Each phase receives its own Lore commit,
   report, dependency/provenance review, and stop.
6. **No learned recall.** Search uses deterministic SQLite/file indexes and
   explicit filters; no embeddings, vector store, semantic ranking, or model
   reflection.
7. **Keep the selected component system.** D0–D12 use the checked-in
   shadcn-svelte component library, Cyan charts, HugeIcons,
   default/translucent menus, and system-following light/dark themes. No phase
   invents a second component library.

   The *palette and typography* were amended by
   [`ADR 0007`](./adr/0007-obsidian-cyan-token-palette.md): Obsidian Cyan
   (dark) / Paper Cyan (light), Inter for prose, JetBrains Mono for code, and
   that ADR's radius scale, replacing this rule's original Nova/Zinc and Geist
   Mono. Governance state is expressed through `--gov-*` tokens, never a
   literal colour. The component library itself is unchanged, which is what
   this rule exists to protect.

## 4. Phase sequence

### Phase D0 — Integrated-workspace authority contract

#### Objective

Define the shared types, trust labels, revision rules, and bounded query shapes
needed by all later surfaces. This phase changes no desktop layout and launches
no processes.

#### Implementation

- Add client-safe identifiers and DTO modules under `src/core/client/` for:
  `WorkItem`, `WorkRelation`, `TrustClass`, `RepositoryState`,
  `ChangeSetSummary`, `ReviewThreadSummary`, `SearchCursor`, and
  `WorkspaceCapability`.
- Extend `src/runtime/client_bridge/` with bounded projections and explicit
  resynchronization for the new DTOs.
- Add reason codes for stale work objects, unavailable capabilities, external
  unverified state, stale diffs, and integration authentication refusal.
- Define maximum counts and byte sizes for work items, files, diff hunks,
  search results, terminal metadata, and integration records.
- Record trust class and provenance for every work item. Trust class is
  runtime-owned and cannot be changed by a frontend field.

#### Acceptance

- JSON round-trip and unknown-field tests cover every DTO.
- A stale revision, unknown work item, unknown trust class, or over-limit page
  is refused with a stable reason code.
- No snapshot contains credentials, environment values, PTY handles, raw
  process IDs intended as authority, or unbounded text.
- Existing A0–C desktop and Ratatui behavior remains unchanged.

### Phase D1 — Persistent work hierarchy and spatial shell

#### Objective

Make project, worktree, session, subagent, council, and parent/child
relationships visible, then support bounded Conversation/Changes and
Conversation/Verify splits as reversible presentation state.

#### Implementation

- Project runtime events into a `workspace → work item → child run` hierarchy.
- Add status rollups for running, blocked, approval required, failed,
  completed-unread, and archived work.
- Replace the desktop’s flat session navigation with grouped Active, Needs
  Attention, Drafts, Completed, and Archive sections.
- Add resizable center/inspector regions and two approved split presets:
  Conversation + Changes and Conversation + Verify.
- Persist only reversible layout preferences. Never persist authority in
  frontend storage.
- Keep narrow windows single-surface and keyboard complete.

#### Acceptance

- A pending child approval raises attention on the child, parent, project, and
  global attention count.
- Completed work remains unread until the user opens or explicitly marks it.
- Split resizing does not duplicate subscriptions, messages, or commands.
- At 1024×700 the complete approval/recovery path remains usable without
  horizontal page scrolling.
- Light and dark system themes, keyboard navigation, focus order, and reduced
  motion pass desktop tests.

### Phase D2 — Governed worktree and child-run control

#### Objective

Expose existing isolated subagent machinery as deliberate work operations and
make parent work with child runs usable from the desktop.

#### Implementation

- Add semantic commands for create worktree, fork work, start child, cancel
  child, preserve branch, settle child, and discard an already-settled
  worktree.
- Reuse `src/runtime/subagent/` rather than adding a desktop worktree engine.
- Require a clean, committed base or a separately approved snapshot strategy;
  never stash or discard user changes automatically.
- Show each child’s directive, branch, base revision, policy ceiling, budget,
  trust class, activity, attention, and settlement result.
- Add compare-child-results and select-for-import projections. Merging remains
  a later governed Git action.

#### Acceptance

- Two children operate in distinct worktrees and cannot write outside their
  contained roots.
- Parent cancellation drains children before parent settlement.
- Crash recovery finds orphaned worktrees and asks rather than guessing whether
  their changes should be retained.
- No child widens policy, budget, credentials, or integration scopes inherited
  from its parent.
- The UI cannot claim merged, applied, or verified merely because a child
  finished.

### Phase D3 — Exact changes, diffs, and line-level review

#### Objective

Replace tool-result approximations with exact changed-file and unified-diff
truth, then let the human send anchored review notes into the governed session.

#### Implementation

- Add `ChangeSet`, `ChangedFile`, `DiffHunk`, `DiffLine`, and
  `ReadBeforeEditEvidence` core/client types.
- Derive proposed, applied, externally imported, and current-working-tree
  change sets separately.
- Capture a stable base object ID and current object ID for every displayed
  diff. Never apply the rendered diff as a patch.
- Add file list, unified diff, hunk navigation, originating tool/event links,
  and exact proposal/applied/verified labels.
- Add review threads anchored to file, side, line, hunk context, and diff
  revision.
- Route “send to mjolnr” as a durable human message/revision request referencing
  the review-thread IDs. Stale anchors remain visible but cannot silently move
  to a different line.

#### Acceptance

- Binary, renamed, deleted, large, non-UTF-8, and truncated files have explicit
  bounded representations.
- A diff whose base changed is marked stale and cannot accept a line note as if
  current.
- Notes survive restart, keep their original anchor, and link to the resulting
  mjolnr response.
- Proposed, applied, imported, and verified states have negative tests against
  false promotion.
- Reviewing a multi-file change requires no external `git diff` invocation by
  the user.

### Phase D4 — Deterministic workspace search

#### Objective

Provide one keyboard-first search across sessions, recorded activity, work
items, review notes, files mentioned by work, reason codes, and available
actions.

#### Implementation

- Add a deterministic SQLite FTS/index projection over bounded, redacted durable
  text. Evaluate FTS5 availability before selecting the final schema.
- Support explicit filters for project, session, work kind, event kind, status,
  provider/model, reason code, file path, and time range.
- Use stable tie-breaking, cursor pagination, and bounded snippets.
- Index append-only records incrementally and rebuild deterministically from
  source events.
- Keep command/action discovery client-local while recorded-work results come
  from Rust.

#### Acceptance

- Rebuilding the index produces the same document set and stable result order.
- Deleted/archived projections cannot leak secrets or records outside the
  selected project scope.
- Arbitrary query input cannot inject SQL or terminal control characters.
- Search remains responsive on a synthetic 100,000-event store and reports the
  measured p50/p95 latency in the report.
- No model call, embedding, semantic score, or learned ranking occurs.

### Phase D5 — Governed local source control

#### Objective

Make repository status, staging, committing, branch operations, and local
integration of selected child work available without bypassing mjolnr’s gates.

#### Implementation

- Add Rust-owned repository status and exact index/worktree projections.
- Add semantic preview commands for stage paths/hunks, unstage, create branch,
  commit, and integrate an explicitly selected child branch.
- Use argument vectors or a dedicated Git library after a dependency
  evaluation; never construct a shell string from UI or model text.
- Persist intent before effect and re-read repository/index state afterward.
- Detect dirty-tree, stale-index, conflict, detached-head, hooks, signing, and
  partial-effect states. Uncertain outcomes require recovery.
- Generate commit-message suggestions as advisory text only; the human selects
  or edits the final message.

#### Acceptance

- Every modifying operation shows the exact repository, base revision, paths or
  hunks, and expected index revision before approval.
- Stale index/repository revisions fail closed.
- Hook failure, conflict, signing failure, and process interruption cannot be
  reported as a successful commit.
- No operation performs automatic stash, reset, clean, force push, or branch
  deletion.
- Post-effect status and object IDs verify every successful claim.

### Phase D6 — GitHub, Linear, Vercel and Supabase task sources

#### Objective

Use GitHub, Linear, Vercel and Supabase as task sources (and GitHub as result
destination) while preserving mjolnr work items as the organizing identity.

#### Implementation

- Define a provider-neutral `TaskSource` and `RemoteChangeRequest` contract.
  Runtime holds sources in `HashMap<IntegrationId, Arc<dyn TaskSource>>`
  (`src/runtime/mod.rs:435`); adding a source is one registration line.
- Deliver GitHub first: repositories, issues, pull requests, review status,
  checks, task-to-session launch, PR creation, and comment/review submission.
- Deliver Linear second: teams, projects, issues, assignees, status, and
  task-to-session launch. `submit_change` remains unavailable — provider-neutral
  contract names a GitHub-style destination Linear does not provide.
- Deliver Vercel third (Phase 6.1): `src/integrations/vercel/` as bounded
  `TaskSource` (`GET /v6/deployments/{id}` on `api.vercel.com`), `TaskId`
  charset validation, bounded reads, `VERCEL_TOKEN` via `Secret`, `Debug`-redacted.
  Read-only — no PR destination, `submit_change` refuses typed `Unavailable`.
- Deliver Supabase fourth (Phase 6.2): `src/integrations/supabase/` as bounded
  `TaskSource` (`GET /v1/projects/{ref}` on `api.supabase.com`), same bounds and
  taxonomy, `SUPABASE_TOKEN`. Read-only, same refusal.
- Treat issue, PR, and comment text as externally supplied data, never owner
  authority. Frame and escape it before it reaches model context.
- Store tokens only in mjolnr’s credential boundary. Snapshots expose account
  labels and scopes, never tokens.
- Require an explicit human action to create work from a task and record the
  source URL, immutable remote ID, fetched revision, and selected policy.

#### Acceptance

- Read-only browsing works before any modifying integration command is enabled.
- A changed remote revision is surfaced before posting, closing, merging, or
  updating status.
- PR creation references a verified local commit and reports the returned remote
  identity; network uncertainty never triggers automatic retry.
- An issue containing prompt-injection text cannot approve tools, widen policy,
  or launch unattended work.
- Revoked/expired credentials fail closed without leaking token material.

### Phase D7 — File explorer and code editor

#### Objective

Add a useful repository file tree and full text editor while keeping human
editing distinct from model-proposed mutations.

#### Implementation

- Add contained, paginated directory/file projections with symlink, binary,
  generated, ignored, large-file, and permission metadata.
- Evaluate CodeMirror 6 and Monaco in a recorded checkpoint before adding an
  editor dependency. Compare bundle size, language support, accessibility,
  worker model, removal cost, and Tauri compatibility.
- Add tabs, go-to-file, find, syntax highlighting, diagnostics display,
  autosave preference, and explicit save.
- Human saves are recorded as operator-controlled edits. Agent suggestions still
  use mjolnr’s ordinary write/edit tool gate.
- Feed saved changes into the D3 change-set model and D5 repository projection.

#### Acceptance

- Containment is rechecked immediately before read and save; symlink escapes are
  refused.
- A stale-on-disk file requires a compare/overwrite decision and is never
  silently replaced.
- Binary and over-limit files open in bounded preview mode, not the editor.
- Saving a file updates Changes without manufacturing agent or verification
  claims.
- Keyboard-only edit, find, save, close, and conflict resolution pass tests.

### Phase D8 — Rust-owned terminal multiplexer

#### Objective

Provide terminal tabs, splits, durable metadata, bounded scrollback, and process
control as an operator surface.

#### Implementation

- Evaluate maintained PTY and terminal-emulation crates against macOS/Linux,
  licence, cancellation, Unicode, resize, and removal cost.
- Host PTYs in Rust. The frontend receives bounded screen/delta data and emits
  typed input/resize/focus/stop commands.
- Support tabs, horizontal/vertical splits, working-directory selection,
  search, copy, restart, and explicit close.
- Scrub child environments and add only declared variables. Never place secrets
  in argv or recorded scrollback.
- Track process lifecycle truthfully; closing a pane and terminating a process
  are separate actions.
- Label these panes operator-controlled unless Phase D9 supplies a more
  specific external-agent class.

#### Acceptance

- Resize, Unicode, ANSI, high-volume output, alternate-screen applications, and
  bounded scrollback pass deterministic tests.
- Cancellation terminates the declared process tree or reports exactly what
  remains; it never claims success from a closed frontend pane.
- A slow renderer cannot grow memory without bound.
- Secret scrubbing and argv tests cover every launched process.
- Terminal functionality remains unavailable to model output as a direct
  command channel.

### Phase D9 — External CLI-agent compatibility

#### Objective

Allow Codex, Claude Code, Gemini CLI, OpenCode, Cursor CLI, and user-defined CLI
profiles to run or attach inside mjolnr’s workspace without false governance
claims.

#### Implementation

- Define a generic CLI-agent profile: executable identity, argument template,
  auth mode, working-directory rules, resume capability, completion signals,
  and declared features.
- Require an explicit executable allowlist and resolved absolute executable
  path. Profiles live in diffable `.mjolnr/` configuration; secrets do not.
- Launch external agents in dedicated worktrees by default and classify them as
  `ExternalUnverified`.
- Record bounded terminal activity, lifecycle, branch, and produced working-tree
  changes separately from the mjolnr transcript.
- Import external changes into D3 for review; stage/commit through D5 only after
  explicit human selection.
- Permit a future `SmedGoverned` adapter only when tests prove all agent tool
  effects are forced through a mjolnr-owned proxy. Marketing/UI copy may not
  imply this before proof.

#### Acceptance

- Unknown executables, unresolved paths, undeclared environment requirements,
  and unsupported resume requests fail closed.
- An external agent can never emit a mjolnr approval, verification, plan
  transition, or governed tool event.
- Stopping one external agent does not terminate sibling panes or mjolnr-native
  sessions.
- Imported changes retain external provenance through review, commit, and PR.
- A generic custom profile works without mjolnr claiming feature parity with
  specifically tested CLIs.

### Phase D10 — Embedded browser and bounded design mode

#### Objective

Provide a per-worktree browser and opt-in UI inspection that can send bounded
visual/DOM context into a governed mjolnr session.

#### Implementation

- Add browser tabs associated with explicit work items and local development
  origins.
- Keep navigation, history, reload, viewport presets, screenshot capture, and
  dev-server association as operator controls.
- Add design mode that selects an element and captures a bounded screenshot,
  sanitized DOM excerpt, computed-style allowlist, page URL, and viewport.
- Inject inspection code only into opted-in allowed origins. Isolate browser
  storage and permissions by workspace where Tauri/WebKit permits it.
- Send captured context as a human attachment to mjolnr; capture itself grants no
  authority to modify files or click further elements.
- Record an explicit feasibility/security checkpoint before supporting remote
  authenticated origins.

#### Acceptance

- Design mode cannot read password values, hidden credential fields, cookies,
  local storage, authorization headers, or cross-origin frames.
- Navigation outside the allowlist disables inspection and explains why.
- Captures are byte-, node-, and text-bounded and redacted before persistence or
  model delivery.
- Browser crashes/reloads do not alter session authority or verification state.
- Local screenshot-to-session-to-change-review works in a packaged macOS build.

### Phase D11 — SSH workspaces and account profiles

#### Objective

Extend the work hierarchy to remote repositories and make provider/integration
account selection explicit without introducing secret rotation or quota
laundering.

#### Implementation

- Add owner-managed SSH host profiles with strict host-key verification,
  username, port, remote root, connection status, and capability discovery.
- Keep private keys, passphrases, tokens, and refresh credentials inside the
  existing secret boundary or delegated OS agent; never serialize them to
  snapshots.
- Reuse D7/D8/D5 surfaces through a remote workspace adapter rather than
  reimplementing editor, terminal, and Git UI.
- Define reconnect, partial-write, remote-process, and uncertain-network
  recovery states.
- Add explicit provider and integration account profiles showing label, scope,
  provider, expiry/health, and current selection.
- Account changes affect future dispatch only unless an explicit handoff
  contract says otherwise. No automatic multi-account rotation.

#### Acceptance

- Unknown or changed host keys fail closed with no “continue automatically”
  path.
- Disconnect during a possible write becomes uncertain recovery, not retry.
- Secrets are absent from argv, logs, SQLite, frontend state, crash reports, and
  terminal recordings.
- Remote and local work use the same trust-class, change-review, Git, and
  verification vocabulary.
- Account switching cannot widen an in-flight run’s policy, budget, tool, or
  credential scope.

### Phase D12 — Daily-driver integration and release checkpoint

#### Objective

Prove the expanded desktop works as one coherent mjolnr journey rather than a
collection of disconnected tools.

#### Implementation

- Unify command palette, work rail, status bar, inspector, notifications,
  onboarding, and recovery across D1–D11.
- Add capability-aware empty states and hide actions whose Rust contracts are
  unavailable.
- Add workspace restore for reversible UI layout, open tabs, and selected work
  without automatically restarting processes or agents.
- Profile large workspaces, long terminal output, large diffs, search indexes,
  browser capture, and multiple child runs.
- Perform accessibility, light/dark theme, reduced-motion, keyboard, offline,
  crash, and packaged-runtime reviews.
- Publish a report containing the tested daily-driver journey and every
  known limitation.

#### Acceptance

- A packaged macOS app completes:
  task import → governed session → child worktree → exact review note → fix →
  verification → stage → commit → PR.
- A second journey completes:
  local browser capture → governed session → file edit → diff review → verify.
- A third journey runs an external CLI agent, preserves its unverified label,
  imports selected changes, and verifies them through mjolnr before commit.
- Restart restores views and durable work truth but does not silently resume
  terminals, browsers, external agents, or uncertain side effects.
- All Rust, desktop unit/component, bridge, architecture, accessibility, and
  packaged smoke suites pass.

## 5. Dependency order

```text
D0 contract
  ├─ D1 hierarchy/splits ─ D2 child control ─ D3 diffs/review ─ D5 Git ─ D6 tasks
  │                                      └─ D4 deterministic search
  ├─ D7 files/editor ───────────────────────────────┘
  ├─ D8 terminal ─ D9 external CLI agents
  ├─ D10 browser/design
  └─ D11 SSH/accounts

D1–D11 ─ D12 daily-driver checkpoint
```

D0 is mandatory first. D1–D4 form the next product-value tranche. D5–D6 form
the shipping/integration tranche. D7–D11 are large bounded workspace
capabilities and may be developed in parallel worktrees only after their shared
D0 contracts settle. D12 is never parallelized with unfinished prerequisite
phases.

## 6. Expected ownership map

These paths are planning targets, not permission to bypass the extraction test:

| Phase | Primary existing/proposed ownership |
|---|---|
| D0 | `src/core/client/`, `src/runtime/client_bridge/`, `tests/architecture.rs` |
| D1 | `desktop/src/routes/+page.svelte`, `desktop/src/lib/components/work/`, `desktop/src/lib/components/inspector/`, desktop component tests |
| D2 | `src/runtime/subagent/`, `src/tools/subagent.rs`, `tests/integration_subagents.rs`, desktop work controls (UI target: [`ADR 0009`](./adr/0009-mockup-layout-as-structural-target.md)) |
| D3 | `src/core/changes.rs`, runtime change capture, `desktop/src/lib/components/surfaces/ChangesSurface.svelte`, change/review persistence tests |
| D4 | `src/store/sqlite/`, workspace-search runtime path, desktop command palette/search results |
| D5 | `src/repository/` (landed), client commands/projections, repository crash/refusal integration tests, and `desktop/src/lib/components/repository/RepositoryControls.svelte` |
| D6 | `src/integrations/github/`, `src/integrations/linear/`, `src/integrations/vercel/`, `src/integrations/supabase/`, `examples/plugins/vercel-deployments/`, `src/store/secrets.rs`, and desktop task/PR surfaces. [`ADR 0009`](./adr/0009-mockup-layout-as-structural-target.md) records why the original mockup did not govern this surface. |
| D7 | `src/core/workspace_files.rs`, `src/workspace_files/`, `Repository::ignored_under`, client commands/projections, desktop Files/Editor surfaces, and stale-save/containment tests. UI boundary: [`ADR 0009`](./adr/0009-mockup-layout-as-structural-target.md); editor dependency: [`ADR 0010`](./adr/0010-codemirror-6-as-the-editor-dependency.md). |
| D8 | `src/runtime/terminal/`, Tauri terminal transport glue, desktop Terminal surface, and PTY/process tests. UI boundary: [`ADR 0009`](./adr/0009-mockup-layout-as-structural-target.md). |
| D9 | proposed `src/runtime/external_agent/`, `.mjolnr/` profile loader, desktop external-agent panes, provenance tests |
| D10 | proposed `src/runtime/browser/`, `desktop/src-tauri/` webview glue, desktop Browser/Design surfaces, redaction/origin tests |
| D11 | proposed `src/runtime/remote/`, `src/store/secrets.rs`, desktop remote/account surfaces, SSH/reconnect tests |
| D12 | desktop shell/onboarding/status integration, packaged smoke tests, report |

If implementation evidence shows a proposed module would combine multiple
reasons to change, split it before adding code and record the final ownership
in that report.

## 7. Risks and mitigations

| Risk | Mitigation |
|---|---|
| The roadmap turns mjolnr into an unfocused IDE | Keep work items and governed review as the organizing flow; ship and assess each phase independently |
| External agents appear to inherit mjolnr guarantees | Runtime-owned trust classes, permanent provenance, import boundary, and negative claim tests |
| Svelte accumulates filesystem/process/network authority | Typed Rust commands only, architecture scans, and no frontend implementation of effects |
| Stale diffs or Git state cause the wrong mutation | Stable object/revision IDs, compare-before-effect, stale refusal, and post-effect reread |
| Task text becomes prompt authority | Treat all remote text as framed untrusted data and require explicit human launch |
| PTY/browser/SSH dependencies expand attack surface | Separate dependency/feasibility checkpoints, least privilege, bounded data, and licence/security review |
| Network or remote interruption duplicates effects | Persist intent before effect, never auto-retry uncertain writes, require recovery |
| Search leaks secrets or becomes semantic recall | Redacted deterministic index, project scoping, rebuild tests, and no embeddings/model ranking |
| Large diffs, terminal streams, or indexes exhaust memory | Hard DTO/output limits, pagination, backpressure, synthetic load tests, and measured p50/p95 evidence |
| Cross-platform behavior diverges | macOS is the first packaged gate; Linux support requires the same phase matrix before being claimed |

## 8. Verification contract

Every phase runs:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo deny check
cd desktop && npm run check
cd desktop && npm test
cd desktop && npm run build
cd desktop/src-tauri && cargo test
```

Each modifying capability also requires:

- negative refusal tests;
- stale-revision tests;
- cancellation and partial-effect tests;
- restart/recovery tests;
- bounded-output and secret-redaction tests;
- an architecture scan proving Svelte owns no side-effect implementation;
- a dependency/provenance update when applicable; and
- a packaged macOS smoke check for the user-visible journey.

## 9. report and commit contract

Each phase stops after one reviewable Lore commit and a report containing:

- intended outcome and exact files changed;
- authoritative DTOs/events/commands added;
- trust class and security boundary;
- negative tests and reason codes;
- dependency purpose, licence, alternatives, and removal cost;
- performance/accessibility/manual evidence;
- known gaps and `Not-tested:` disclosure; and
- whether the next phase is unblocked.

No phase may advance because its missing evidence is documented as acceptable.
