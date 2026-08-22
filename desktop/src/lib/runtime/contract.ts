// TypeScript interfaces mirroring mjolnr's Rust Client DTO Contract (src/core/client/)

export type ClientPolicy = 'read-only' | 'ask' | 'workspace-write' | 'full-auto';
export type ClientToolOutcome = 'ok' | 'refused' | 'failed';
export type ClientApprovalDecision = 'deny' | 'approve-once' | 'approve-exact-for-session';
export type ClientApprovalResolution = 'deny' | 'approve-once' | 'approve-exact-for-session' | 'auto-by-policy';
export type ClientResumeChoice = 'compact' | 'new-from-handoff' | 'full';
export type ClientRecoveryDecision = 'abandon-and-continue' | 'end-session';
export type ClientFinishReason = 'stop' | 'tool-calls' | 'incomplete' | 'cancelled' | 'handoff' | 'quota-drained';

export interface ClientUsage {
  inputTokens: number;
  outputTokens: number;
}

export interface ClientBudget {
  providerTurns: number;
  maxProviderTurns: number;
  toolCalls: number;
  maxToolCalls: number;
}

export interface ClientQuotaWindow {
  label: string;
  usedFraction: number;
  resetsAt?: string;
  /**
   * Whether this window's pool actually covers the model in use, computed on
   * the Rust side (`convert.rs::pool_covers_model`) rather than re-derived
   * here — window-to-model matching is backend-owned state, not a frontend
   * status decision.
   */
  isRelevant: boolean;
}

export interface ClientQuota {
  provider: string;
  windows: ClientQuotaWindow[];
}

/** Mirrors `ProviderConnectionState` in src/core/runtime.rs. */
export type ClientProviderConnectionState =
  | 'disconnected'
  | 'discovering'
  | 'connected'
  | 'needsReauth'
  | 'unavailable';

export interface ClientAccount {
  provider: string;
  state: ClientProviderConnectionState;
  detail?: string;
}

/** Mirrors `SkillScope` in src/core/context.rs. */
export type ClientPersonaScope = 'project' | 'user';

export interface ClientPersonaSummary {
  name: string;
  description?: string;
  scope: ClientPersonaScope;
}

export interface ClientRoute {
  name: string;
  roles: string[];
  provider: string;
  model: string;
  persona?: string;
}

export interface ClientToolCallRef {
  id: string;
  name: string;
}

export type ClientMessage =
  | { kind: 'user'; id: string; text: string; textTruncated: boolean; at?: string | null }
  | { kind: 'system'; id: string; text: string; textTruncated: boolean; at?: string | null }
  | {
      kind: 'assistant';
      id: string;
      text: string;
      textTruncated: boolean;
      provider?: string;
      model?: string;
      toolCalls: ClientToolCallRef[];
      at?: string | null;
    }
  | {
      kind: 'tool';
      id: string;
      name: string;
      outcome: ClientToolOutcome;
      reasonCode?: string;
      detail: string;
      detailTruncated: boolean;
      at?: string | null;
    };

export interface ClientApproval {
  id: string;
  toolName: string;
  tier: string;
  preview: string;
}

export type ClientRecovery =
  | { state: 'clean' }
  | {
      state: 'required';
      run: string;
      kind: string;
      summary: string;
      effectIsCertain: boolean;
      toolName?: string;
      preview?: string;
    };

export interface ClientModelChoice {
  provider: string;
  model: string;
  displayName: string;
}

export interface ClientResumeAdvice {
  warning: string;
  estimatedFullResumeTokens: number;
  hasHandoff: boolean;
}

export interface ClientContextDiagnostic {
  code: string;
  detail: string;
}

export type ClientOnboardingFileAction = 'write' | 'preserve';

export interface ClientOnboardingDraft {
  root: string;
  soul: string;
  userProfile?: string;
}

export interface ClientOnboardingFileStatus {
  path: string;
  action: ClientOnboardingFileAction;
}

export interface ClientOnboardingPreview {
  root: string;
  files: ClientOnboardingFileStatus[];
}

export type ClientReviewVerdict = 'approve' | 'iterate' | 'reject';

export interface ClientPlanStep {
  index: number;
  title: string;
  description: string;
}

export interface ClientPlanProposal {
  planId: string;
  revisionId: number;
  title: string;
  summary: string;
  steps: ClientPlanStep[];
  proposedAt: string;
}

export interface ClientPlanReview {
  planId: string;
  revisionId: number;
  reviewer: string;
  verdict: ClientReviewVerdict;
  feedback: string;
  reviewedAt: string;
}

export interface ClientPlanApproval {
  planId: string;
  revisionId: number;
  approver: string;
  decision: ClientReviewVerdict;
  note?: string;
  approvedAt: string;
}

export interface ClientPlanHandoff {
  planId: string;
  revisionId: number;
  handoffNote: string;
  createdAt: string;
}

export interface ClientPlanQuestion {
  id: string;
  prompt: string;
  options: string[];
  isMultiSelect: boolean;
  createdAt: string;
}

export interface ClientPlanQuestionAnswer {
  questionId: string;
  selectedOptions: string[];
  freeformText?: string;
  answeredAt: string;
}

export type ClientPlanStage =
  | 'Idle'
  | { QuestionPending: { question: ClientPlanQuestion } }
  | { Proposed: { proposal: ClientPlanProposal } }
  | { Reviewed: { proposal: ClientPlanProposal; reviews: ClientPlanReview[] } }
  | { Approved: { proposal: ClientPlanProposal; approval: ClientPlanApproval } }
  | { IterateRequested: { proposal: ClientPlanProposal; feedback: string } }
  | { Rejected: { proposal: ClientPlanProposal; reason: string } }
  | { Handoff: { proposal: ClientPlanProposal; handoff: ClientPlanHandoff } };

export interface ClientPlanWorkflow {
  planId: string;
  activeRevision?: number;
  stage: ClientPlanStage;
  proposals: ClientPlanProposal[];
  reviews: ClientPlanReview[];
  approvals: ClientPlanApproval[];
  handoffs: ClientPlanHandoff[];
}

export interface ClientCouncilReview {
  reviewId: string;
  question: string;
  contributions: ClientCouncilContribution[];
  roundsConducted: number;
  artifact?: ClientCouncilArtifact;
  findings: ClientCouncilFinding[];
  /**
   * The amendment composed from accepted findings, when a human asked for one.
   * A draft for the editor — not a change on disk. Saving it goes through the
   * ordinary governed save path, which re-checks the digest independently.
   */
  amendment?: ClientCouncilAmendment | null;
}

export interface ClientCouncilAmendment {
  reviewId: string;
  path: string;
  sourceDigest: string;
  acceptedFindings: number;
  text: string;
}

export interface ClientCouncilArtifact {
  path: string;
  sourceDigest: string;
}

export interface ClientCouncilContribution {
  role: string;
  proposal: string;
  /** The runtime serializes `Option<String>` as `null`; absent dissent arrives as `null`. */
  critique?: string | null;
}

export type ClientCouncilDisposition = 'accept' | 'reject' | 'defer';

export interface ClientCouncilFinding {
  id: string;
  section: string;
  title: string;
  positions: ClientCouncilPosition[];
  disposition?: ClientCouncilDispositionRecord;
}

export interface ClientCouncilPosition {
  role: string;
  response: string;
  /** The runtime serializes `Option<String>` as `null`; absent dissent arrives as `null`. */
  critique?: string | null;
}

export interface ClientCouncilDispositionRecord {
  disposition: ClientCouncilDisposition;
  /** The runtime serializes `Option<String>` as `null`; an absent note arrives as `null`. */
  note?: string | null;
  decidedAt: string;
}

export interface ClientSnapshot {
  revision: number;
  session?: string;
  provider?: string;
  model?: string;
  workspaceRoot?: string;
  policy: ClientPolicy;
  runActive: boolean;
  usage: ClientUsage;
  budget: ClientBudget;
  /** Absent until a provider has reported quota at least once this session. */
  quota?: ClientQuota;
  messages: ClientMessage[];
  messagesOmitted: number;
  pendingApproval?: ClientApproval;
  recovery: ClientRecovery;
  storeFailure?: string;
  /** Rust-owned context/configuration diagnostics; absent only for old snapshots. */
  contextDiagnostics?: ClientContextDiagnostic[];
  models: ClientModelChoice[];
  resumeAdvice?: ClientResumeAdvice;
  sessions: ClientSessionSummary[];
  /**
   * The session's explicit persona override, if any. Absent means the active
   * route's own persona applies instead, not that no persona exists at all.
   */
  activePersona?: string;
  /** Personas discovered under .mjolnr/personas/ and the user config dir. */
  personas: ClientPersonaSummary[];
  /** Soul/profile file names in effect. Names only, never file content. */
  souls: string[];
  /** The provider/model routing table. Read-only projection. */
  routes: ClientRoute[];
  /** The latest completed advisory council distribution, if one ran. */
  council?: ClientCouncilReview | null;
  /** One entry per provider mjolnr is configured to talk to (§E2). */
  accounts: ClientAccount[];
  plan?: ClientPlanWorkflow;
  changes?: ClientChangeSet;
  /**
   * Always present. The three states a reader must tell apart — no project,
   * unreadable, read at a moment — live in `freshness`, so an absent field
   * would collapse them into one silence.
   */
  repository: ClientRepositoryState;
  /**
   * Line notes pinned to the diff, oldest first (Phase D3). Always present: an
   * empty projection means "no notes", which is a different statement from the
   * absence a missing field would make.
   */
  reviewThreads: ClientBoundedProjection<ClientReviewThreadSummary>;
  memory?: ClientMemorySummary;
  plugins?: ClientPluginSummary[];
  fleet?: ClientFleetSummary;
  preview?: ClientPreviewState;
}

export interface ClientProjectSummary {
  contextId: string;
  root: string;
  selected: boolean;
  session?: string;
  runActive: boolean;
  approvalPending: boolean;
  recoveryRequired: boolean;
}

export type ClientViewportPreset = 'desktop' | 'tablet' | 'mobile';

export interface ClientPreviewViewport {
  preset?: ClientViewportPreset | null;
  width: number;
  height: number;
  zoom: number;
  locale?: string | null;
}

export interface ClientPreviewState {
  url?: string | null;
  active: boolean;
  viewport: ClientPreviewViewport;
  autoReload: boolean;
}

export interface ClientMemorySummary {
  rulesCount: number;
  userProfilePresent: boolean;
  factsCount?: number | null;
  episodesCount?: number | null;
  projectionError?: string | null;
  rulesError?: string | null;
  ruleNames: string[];
}

export interface ClientPluginSummary {
  name: string;
  version: string;
  publisher: string;
  description: string;
  toolCount: number;
  hookCount: number;
  requiredCredentials: string[];
  sourceUrl?: string | null;
}

export type ClientFleetAgentStatus =
  | 'idle'
  | 'running'
  | 'completed'
  | { failed: { reason: string } };

export interface ClientFleetAgentSummary {
  childSessionId: string;
  shortName: string;
  role?: string | null;
  status: ClientFleetAgentStatus;
  latestActivity: string;
  feed: string[];
  worktreeBranch?: string | null;
}

export interface ClientFleetSummary {
  visible: boolean;
  activeCount: number;
  agents: ClientFleetAgentSummary[];
}

export type ClientTerminalStatus = 'running' | 'stopping' | 'exited' | 'failed';

export interface ClientTerminalSnapshot {
  id: string;
  status: ClientTerminalStatus;
  cwd: string;
  screen: string;
  rows: number;
  cols: number;
  scrollbackRows: number;
  scrollbackOffset: number;
  screenTruncated: boolean;
  exitCode?: number;
  detail?: string;
}

export interface ClientTerminalInput {
  id: string;
  data: string;
}

export interface ClientTerminalResize {
  id: string;
  rows: number;
  cols: number;
}

export interface ClientTerminalScroll {
  id: string;
  rows: number;
}

export interface ClientTerminalSearchMatch {
  scrollbackOffset: number;
  text: string;
}

export interface ClientTerminalSearchResult {
  matches: ClientTerminalSearchMatch[];
  truncated: boolean;
}

export type ClientTerminalSplitDirection = 'horizontal' | 'vertical';

export interface ClientTerminalLayout {
  primaryCwd: string;
  splitDirection?: ClientTerminalSplitDirection;
  secondaryCwd?: string;
}

export type ClientGraphDirection = 'imports' | 'importers' | 'both';

export type ClientGraphBuildPhase = 'idle' | 'building' | 'ready' | 'failed';

export interface ClientGraphStatus {
  phase: ClientGraphBuildPhase;
  detail: string;
  filesScanned: number;
  filesTotal: number;
  nodes: number;
  edges: number;
}

export interface ClientGraphLanguageCapability {
  language: string;
  files: number;
  imports: boolean;
  symbols: boolean;
  callGraph: boolean;
  resolver: string;
  extraction: string;
}

export interface ClientGraphQuery {
  path?: string | null;
  depth: number;
  direction: ClientGraphDirection;
  search?: string | null;
}

export interface ClientGraphSymbol {
  name: string;
  kind: string;
  line: number;
}

export interface ClientGraphNode {
  path: string;
  language: string;
  distance?: number | null;
  degree: number;
  community?: number | null;
  communitySize: number;
  isArticulationPoint: boolean;
  inCycle: boolean;
  imports: string[];
  importers: string[];
  symbols: ClientGraphSymbol[];
}

export interface ClientGraphEdge {
  from: string;
  to: string;
  relation: string;
  provenance: string;
  confidenceBps: number;
}

export interface ClientGraphSummary {
  filesScanned: number;
  externalImports: number;
  unresolvedImports: number;
  filesSkipped: number;
  filesTooLarge: number;
  nonParsedEdges: number;
  communities: number;
  articulationPoints: number;
  cycleNodes: number;
  unsupportedLanguages: string[];
  languages: ClientGraphLanguageCapability[];
}

export interface ClientGraphPage {
  query: ClientGraphQuery;
  nodes: ClientGraphNode[];
  edges: ClientGraphEdge[];
  summary: ClientGraphSummary;
  truncated: boolean;
}

// ---------------------------------------------------------------------------
// Board contract (Phase E5, step 3)
// Mirrors: src/core/client/board.rs
// ---------------------------------------------------------------------------

export interface ClientBoardNode {
  id: string;
  kind: 'decision' | 'implementation';
  provenance: ClientTrustClass;
  label: string;
}

export interface ClientImportedTask {
  boardId: string;
  integration: string;
  remoteId: string;
  sourceUrl: string;
  fetchedRevision: string;
  title: string;
  state: string;
}

export interface ClientImportedAct {
  actId: string;
  itemBoardId: string;
  kind: string;
  expectedRevision: string;
  headBranch: string;
  baseBranch: string;
  outcome: string;
  remoteUrl?: string | null;
}

export interface ClientFoggedNode {
  node: ClientBoardNode;
  waitsOn: ClientBoardNode[];
}

export interface ClientBoardOverview {
  importedTasks: ClientImportedTask[];
  importedActs: ClientImportedAct[];
  frontier: ClientBoardNode[];
  fog: ClientFoggedNode[];
  settled: ClientBoardNode[];
  cycles: ClientBoardNode[][];
}

export interface ClientSessionSummary {
  id: string;
  title: string;
  projectRoot: string;
  status: string;
  rollupStatus: ClientRollupStatus;
  provider?: string;
  model?: string;
  updatedAt: string;
  eventCount: number;
  leased: boolean;
  parent?: string;
}

/**
 * Session rollup vocabulary (Phase D1). Mirrors `ClientRollupStatus` in
 * `src/core/client/types.rs`. The runtime owns production; the frontend only
 * groups on these values. There is no `archived` value: `SessionStatus` has
 * no archived state, so an archive group could never be populated honestly.
 */
export type ClientRollupStatus = 'running' | 'active' | 'draft' | 'completed';

export interface ClientRecoveryWork {
  run: string;
  kind: string;
  summary: string;
  effectIsCertain: boolean;
  toolName?: string;
  preview?: string;
}

export type ClientEvent =
  | { activity: 'sessionStarted'; session: string; provider: string; model: string }
  | { activity: 'runStarted'; run: string }
  | { activity: 'textDelta'; run: string; text: string; textTruncated: boolean }
  | { activity: 'reasoningDelta'; run: string; text: string; textTruncated: boolean }
  | { activity: 'toolAssembling'; run: string; name: string }
  | { activity: 'toolProposed'; run: string; approval?: string; name: string; preview: string }
  | { activity: 'approvalResolved'; run: string; approval: string; decision: ClientApprovalResolution }
  | { activity: 'toolCompleted'; run: string; name: string; outcome: ClientToolOutcome; reasonCode?: string }
  | { activity: 'runFinished'; run: string; reason: ClientFinishReason }
  | { activity: 'runFailed'; run: string; code: string; detail: string; detailTruncated: boolean }
  | { activity: 'policyChanged'; policy: ClientPolicy }
  | { activity: 'modelChanged'; provider: string; model: string }
  | {
      activity: 'fileSaved';
      path: string;
      observedDigest: string;
      newDigest: string;
      sizeBytes: number;
    }
  | { activity: 'subagentActivity'; child: string; label: string }
  | {
      activity: 'subagentSpawned';
      child: string;
      directive: string;
      directiveTruncated: boolean;
      branch: string;
      worktree: string;
    }
  | { activity: 'recoveryRequired'; work: ClientRecoveryWork }
  | { activity: 'recoveryResolved'; decision: ClientRecoveryDecision }
  | { activity: 'sessionEnded' };

export type ClientUpdate =
  | { type: 'snapshot'; snapshot: ClientSnapshot }
  | { type: 'event'; sequence: number; event: ClientEvent }
  | { type: 'resync'; missed: number; snapshot: ClientSnapshot }
  | { type: 'closed' };

export interface ContextTaggedUpdate {
  contextId: string;
  sequence: number;
  update: ClientUpdate;
}

export type ClientCommand =
  | { type: 'openProject'; root: string }
  | { type: 'cloneProject'; source: string; destination: string }
  /**
   * Ask what git says about the open project now. No fields: the runtime reads
   * the project it already has open, and accepting a root here would be a
   * second way to point mjolnr at a directory, bypassing every refusal
   * `openProject` applies.
   */
  | { type: 'refreshRepository' }
  // D7 save is operator-controlled: the digest is the full-file version the
  // client read, and the runtime refuses a stale overwrite.
  | { type: 'saveFile'; path: string; expectedDigest: string; text: string }
  | { type: 'createSession'; provider: string; model: string }
  /**
   * Switch the open session's route while idle. The runtime refuses with a
   * typed reason when a run is active or the target is not connected.
   */
  | { type: 'selectModel'; provider: string; model: string }
  | { type: 'resumeSession'; session: string }
  | { type: 'resolveResume'; choice: ClientResumeChoice }
  | { type: 'sendMessage'; text: string }
  | { type: 'cancelRun' }
  | { type: 'resolveApproval'; approval: string; decision: ClientApprovalDecision }
  | { type: 'resolveRecovery'; decision: ClientRecoveryDecision }
  | { type: 'setPolicy'; policy: ClientPolicy }
  | { type: 'askPlanQuestion'; planId: string; prompt: string; options: string[]; isMultiSelect: boolean }
  | { type: 'answerPlanQuestion'; planId: string; questionId: string; selectedOptions: string[]; freeformText?: string }
  | { type: 'proposePlan'; planId: string; revision: number; title: string; summary: string; steps: ClientPlanStep[] }
  | { type: 'reviewPlan'; planId: string; revision: number; reviewer: string; verdict: ClientReviewVerdict; feedback: string }
  | { type: 'approvePlan'; planId: string; revision: number; decision: ClientReviewVerdict; note?: string }
  | { type: 'handoffPlan'; planId: string; revision: number; note: string }
  | { type: 'endSession' }
  | { type: 'releaseSession' }
  | { type: 'reclaimSession'; session: string }
  | { type: 'requestSnapshot' }
  | { type: 'refreshCredentials' }
  | { type: 'createWorktree'; name: string; baseRevision: string }
  | { type: 'forkWork'; name: string; baseRevision: string }
  // policyCeiling is optional: omitted means "inherit the parent's policy
  // unchanged"; a value may only lower the ceiling — children inherit less,
  // never more (AGENTS §11.4), and the runtime clamps at execution time.
  | { type: 'startChild'; name: string; directive: string; policyCeiling?: ClientPolicy; budget?: number }
  | { type: 'cancelChild'; name: string }
  | { type: 'preserveBranch'; name: string }
  | { type: 'settleChild'; name: string }
  | { type: 'discardSettledWorktree'; name: string }
  // Phase D5 repository family. Mirrors: src/core/client/command.rs.
  // Every value becomes an argv element of a git invocation, so the Rust
  // bridge validates each one and refuses with SCHEMA_INVALID; the frontend
  // must not assume a permissive server.
  | { type: 'stagePaths'; paths: string[] }
  // stageHunks is on the wire but the runtime refuses it with
  // WORKSPACE_CAPABILITY_UNAVAILABLE: naming a hunk stably needs the D3 diff
  // identity, and staging by ordinal would be an unsound write.
  | { type: 'stageHunks'; path: string; hunkIndices: number[] }
  | { type: 'unstage'; paths: string[] }
  | { type: 'createBranch'; name: string; baseRevision: string }
  // message is the human's; expectedIndexRevision is the index the human saw
  // when they approved. A mismatch is refused with WORKSPACE_STALE_REVISION.
  | { type: 'commit'; message: string; expectedIndexRevision: string }
  // message is required and human-supplied: mjolnr never authors the merge
  // commit's record.
  | { type: 'integrateChildBranch'; name: string; message: string; expectedHead: string }
  // Fetch from the configured upstream remote. Inert and human-initiated: it
  // touches only remote-tracking refs, never the working tree.
  | { type: 'fetch' }
  // Push the current branch's HEAD to its configured upstream. Human-initiated
  // from the preview; the model never self-approves a push. expectedHead is the
  // HEAD the human saw; a mismatch is refused with WORKSPACE_STALE_REVISION.
  | { type: 'push'; expectedHead: string }
  // Merge the branch's configured upstream into it — the merge half of "pull"
  // (fetch and merge are two evidenced acts). Human-initiated from the
  // preview; message is human-supplied and consumed only when the merge
  // creates a commit — a fast-forward creates none, exactly as `git pull`
  // does. expectedHead is the HEAD the human saw; a mismatch is refused with
  // WORKSPACE_STALE_REVISION. A branch already containing the upstream tip is
  // a verified no-op.
  | { type: 'integrateUpstream'; message: string; expectedHead: string }
  | { type: 'rebase'; onto: string; expectedHead: string }
  | { type: 'abortRebase' }
  // Phase D6 integration family. Mirrors: src/core/client/command.rs.
  | { type: 'fetchTask'; source: string; taskId: string }
  | { type: 'fetchTasks'; source: string; taskIds: string[] }
  | { type: 'submitChange'; source: string; request: ClientRemoteChangeRequest }
  // Phase D3 review family. Mirrors: src/core/client/command.rs.
  //
  // captureDigest is required, not optional: it is the diff revision the human
  // was looking at, and an opt-in staleness guard is not a guard. A mismatch is
  // refused with WORKSPACE_STALE_DIFF — the note is never moved to whatever
  // occupies that line now.
  //
  // There is no hunkHeader field. The runtime reads the hunk context out of its
  // own capture, so a client cannot record a note against a diff that never
  // existed.
  | { type: 'addReviewNote'; path: string; side: 'old' | 'new'; line: number; captureDigest: string; body: string }
  | { type: 'addReviewComment'; threadId: string; body: string }
  | { type: 'sendReviewNotes'; threadIds: string[] }
  | {
      type: 'resolveCouncilFinding';
      reviewId: string;
      findingId: string;
      disposition: ClientCouncilDisposition;
      note?: string;
    }
  // Composing an amendment writes nothing. It marks the artifact up with the
  // findings a human accepted and hands the draft back for a human to save.
  | { type: 'proposeCouncilAmendment'; reviewId: string }
  | { type: 'rollbackToCheckpoint'; targetSequence: number; expectedHead?: string };

// ---------------------------------------------------------------------------
// Integrated-workspace authority contract (Phase D0)
// Mirrors: src/core/client/workspace.rs
// ---------------------------------------------------------------------------

export type ClientTrustClass = 'mjolnrGoverned' | 'operatorControlled' | 'externalUnverified';

export interface ClientWorkItemProvenance {
  source: string;
  fetchedAt: string;
  trust: ClientTrustClass;
}

export interface ClientWorkItem {
  id: string;
  title: string;
  titleTruncated: boolean;
  state: string;
  provenance: ClientWorkItemProvenance;
  revision: number;
}

export type ClientWorkRelationKind = 'parentChild' | 'references' | 'blocks' | 'duplicates' | 'unknown';

export interface ClientWorkRelation {
  sourceId: string;
  targetId: string;
  kind: ClientWorkRelationKind;
  trust: ClientTrustClass;
}

/**
 * Where the branch stands against its remote-tracking ref (ADR 0008).
 *
 * Every variant except `unknown` describes the ref as it stood when mjolnr last
 * saw the remote — NOT the remote now. Computing this touches no network;
 * learning whether the remote has moved since would, and no read path may.
 *
 * `unknown` means there is no upstream to compare against, or git would not
 * answer. It does not mean "mjolnr did not look".
 *
 * **`synced` is a trap.** It means "level with the ref last seen". Never render
 * it as a bare "synced", and never in the verified colour: being level with a
 * ref fetched an hour ago is not a verified state, and
 * `docs/tauri-design-system.md` forbids a component claiming one. Render the
 * as-of qualifier alongside it — see `remoteSyncAsOf`.
 */
export type ClientRepositorySyncState =
  | { type: 'unknown' }
  | { type: 'ahead'; count: number }
  | { type: 'behind'; count: number }
  | { type: 'diverged'; ahead: number; behind: number }
  | { type: 'synced' };

/**
 * Whether the repository was read, and if so at what moment (Phase D5).
 *
 * There is deliberately no `fresh` or `upToDate` variant. mjolnr refreshes on
 * explicit triggers and nothing watches the filesystem, so a surface can say
 * what git reported and when it was asked — never that the answer is still
 * true. Render `capturedAt` as the freshness marker; do not translate it into
 * a claim of currency.
 */
export type ClientRepositoryFreshness =
  | { type: 'noProject' }
  | { type: 'unavailable'; code: string; detail: string }
  | { type: 'capturedAt'; trigger: ClientRepositoryRefreshTrigger; sequence: number };

/** Stable identifiers, matching `core::repository::RefreshTrigger::as_str`. */
export type ClientRepositoryRefreshTrigger =
  | 'projectOpened'
  | 'repositoryCommand'
  | 'fileSave'
  | 'toolWrite'
  | 'requested';

export interface ClientRepositoryState {
  branch?: string;
  head?: string;
  /**
   * Echo into a `commit` command's `expectedIndexRevision`. Advisory: the
   * runtime re-reads and compares, so a stale value here becomes a refusal,
   * never a wrong commit.
   */
  indexRevision?: string;
  dirtyCount: number;
  dirtyCountTruncated: boolean;
  stagedFiles: string[];
  modifiedFiles: string[];
  untrackedFiles: string[];
  /** Conflicted paths. Never offer these as ordinary stageable changes. */
  unmergedFiles: string[];
  /** True only when git reports an explicit in-progress rebase state. */
  rebaseInProgress: boolean;
  pathsTruncated: boolean;
  remoteSync: ClientRepositorySyncState;
  /**
   * When mjolnr last saw the remote, for qualifying `remoteSync`.
   *
   * Often absent — a fresh clone writes its tracking ref without a reflog
   * entry. The as-of qualifier is rendered WHETHER OR NOT this is present; the
   * timestamp only sharpens it. Do not make the qualifier conditional on it.
   */
  remoteSyncAsOf?: string;
  freshness: ClientRepositoryFreshness;
  trust: ClientTrustClass;
}

export interface ClientRepositoryHistoryEntry {
  revision: string;
  author: string;
  authoredAt: string;
  subject: string;
}

export interface ClientRepositoryHistory {
  entries: ClientRepositoryHistoryEntry[];
  hasMore: boolean;
  limit: number;
  trust: ClientTrustClass;
}

export type ClientWorkspaceFileContent =
  | { type: 'sniffed'; binary: boolean; generated: boolean }
  | { type: 'oversized' }
  | { type: 'unreadable' }
  | { type: 'notAFile' };

export interface ClientDirectoryEntry {
  name: string;
  path: string;
  kind: 'directory' | 'file' | 'other' | string;
  symlink?: { target?: string; escaping: boolean };
  content:
    | ClientWorkspaceFileContent
    | { type: string; [key: string]: unknown };
  sizeBytes?: number;
  ignored: boolean;
  writable: boolean;
}

export interface ClientDirectoryPage {
  path: string;
  page: number;
  entries: {
    items: ClientDirectoryEntry[];
    limit: number;
    total?: number;
    truncated: boolean;
    reasonCode?: string;
  };
  hasMore: boolean;
  trust: ClientTrustClass;
}

export type ClientFileMode =
  | { type: 'editable'; text: string; textTruncated: boolean }
  | { type: 'preview'; reason: string; excerpt: string; excerptTruncated: boolean };

export interface ClientFileOpen {
  path: string;
  mode: ClientFileMode;
  digest: string;
  sizeBytes?: number;
  writable: boolean;
  trust: ClientTrustClass;
}

/** Human-controlled editor preferences persisted under the workspace `.mjolnr/` directory. */
export interface ClientEditorPreferences {
  autosave: boolean;
}

/**
 * The repository state for "no project is open".
 *
 * Exported so a snapshot fixture is one line rather than a nine-field literal
 * repeated across every test. `externalUnverified` is deliberate and must not
 * be "tidied" to `mjolnrGoverned`: an empty state is the absence of a governed
 * observation, not a governed observation of an empty repository.
 */
export const NO_PROJECT_REPOSITORY: ClientRepositoryState = {
  dirtyCount: 0,
  dirtyCountTruncated: false,
  stagedFiles: [],
  modifiedFiles: [],
  untrackedFiles: [],
  unmergedFiles: [],
  rebaseInProgress: false,
  pathsTruncated: false,
  remoteSync: { type: 'unknown' },
  freshness: { type: 'noProject' },
  trust: 'externalUnverified'
};

/**
 * The review-thread projection for "no notes taken".
 *
 * Exported for the same reason `NO_PROJECT_REPOSITORY` is: a fixture should be
 * one line, not a five-field literal repeated across every test. `limit` mirrors
 * MAX_REVIEW_THREADS_PER_ITEM in src/core/client/workspace.rs.
 */
export const NO_REVIEW_THREADS: ClientBoundedProjection<ClientReviewThreadSummary> = {
  items: [],
  limit: 100,
  total: 0,
  truncated: false
};

export interface ClientChangeSetSummary {
  fileCount: number;
  fileCountTruncated: boolean;
  insertions: number;
  deletions: number;
  trust: ClientTrustClass;
  revision: number;
}

/**
 * Where a review note is pinned, exactly as it was pinned (Phase D3).
 *
 * Every field is the value recorded when the note was taken. Nothing recomputes
 * one, which is what §D3's "stale anchors remain visible but cannot silently
 * move to a different line" means in practice: a stale thread arrives with its
 * original line, and `anchorStale` says so beside it.
 */
export interface ClientReviewAnchor {
  path: string;
  side: 'old' | 'new';
  line: number;
  hunkHeader: string;
  /** The diff revision the note was taken against. */
  captureDigest: string;
  baseObjectId?: string;
}

export interface ClientReviewComment {
  body: string;
  bodyTruncated: boolean;
  createdAt: string;
}

/**
 * `status` is `'open'` or `'sent'` and nothing else. There is deliberately no
 * `'resolved'`, `'applied'`, or `'verified'`: mjolnr cannot know a note was
 * addressed, so no surface may render as if it does.
 */
export interface ClientReviewThreadSummary {
  id: string;
  status: string;
  commentCount: number;
  commentCountTruncated: boolean;
  trust: ClientTrustClass;
  anchor: ClientReviewAnchor;
  /** True when the diff has moved since the note was taken — or when nothing is captured to compare against, which is not the same as "still current". */
  anchorStale: boolean;
  comments: ClientReviewComment[];
  /** The `ClientMessage` id mjolnr answered with, once a sent request produced one. */
  responseMessageId?: string;
}

export interface ClientSearchCursor {
  opaqueToken: string;
  pageSize: number;
  totalKnown?: number;
  depth: number;
}

export interface ClientWorkspaceSearchFilter {
  query?: string;
  projectId?: string;
  sessionId?: string;
  workKind?: string;
  eventKind?: string;
  status?: string;
  providerModel?: string;
  reasonCode?: string;
  filePath?: string;
  timeStart?: string;
  timeEnd?: string;
  cursor?: string;
  limit: number;
}

export interface ClientWorkspaceSearchResult {
  sessionId: string;
  eventId: string;
  sequence: number;
  matchSnippet: string;
  occurredAt: string;
}

export interface ClientWorkspaceSearchPage {
  items: ClientWorkspaceSearchResult[];
  nextCursor?: string;
}

export interface ClientWorkspaceCapability {
  key: string;
  available: boolean;
  reason?: string;
}

export interface ClientBoundedProjection<T> {
  items: T[];
  limit: number;
  total?: number;
  truncated: boolean;
  reasonCode?: string;
}

export interface ClientWorkspaceRefusal {
  code: string;
  message: string;
  attemptedRevision?: number;
  currentRevision?: number;
}

// ---------------------------------------------------------------------------
// Exact changes and line-level review (Phase D3)
// Mirrors: src/core/changes.rs
// ---------------------------------------------------------------------------

export type ClientChangeState = 'proposed' | 'applied' | 'externallyImported' | 'currentWorkingTree';
export type ClientFileStatus = 'added' | 'modified' | 'deleted' | 'renamed';
export type ClientLineKind = 'unchanged' | 'added' | 'removed';

export interface ClientDiffLine {
  kind: ClientLineKind;
  content: string;
  oldLineNumber?: number;
  newLineNumber?: number;
}

export interface ClientDiffHunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  header: string;
  lines: ClientDiffLine[];
}

/**
 * Why a changed file does or does not carry reviewable text.
 *
 * `binary` (git declined to diff it) and `undecodable` (git handed back bytes
 * that are not UTF-8) are different problems with different answers, so they
 * stay distinct — but they are mutually exclusive, which is why this is one
 * field rather than two booleans that could claim both at once.
 */
export type ClientFileContent = 'text' | 'binary' | 'undecodable';

export interface ClientChangedFile {
  path: string;
  status: ClientFileStatus;
  hunks: ClientDiffHunk[];
  content: ClientFileContent;
  isLarge: boolean;
  isTruncated: boolean;
  oldPath?: string;
}

export interface ClientChangeSet {
  baseObjectId?: string;
  currentObjectId?: string;
  files: ClientChangedFile[];
  state: ClientChangeState;
  readEvidence: ClientReadBeforeEditEvidence[];
  /** SHA-256 of the exact diff bytes shown. Content identity, not a git object id. */
  captureDigest: string;
  /** Matches `repository.freshness.sequence` for the same refresh. */
  captureSequence: number;
  /** Files were dropped at the projection bound, or git's own output was cut. */
  filesTruncated: boolean;
  /** Untracked paths that exist but were not diffed. Named, never dropped. */
  undiffedUntracked: string[];
}

export interface ClientReadBeforeEditEvidence {
  path: string;
  readRevision: string;
  toolEventId: string;
}

// ---------------------------------------------------------------------------
// External Task Integrations (Phase D6)
// Mirrors: src/integrations/mod.rs
// ---------------------------------------------------------------------------

// Only ClientRemoteChangeRequest lives here, because it is the only D6 type a
// command actually carries. The earlier draft also declared ClientTask and
// ClientTaskSource: neither had a Rust counterpart and nothing produced or
// consumed them, which is how a "contract" file starts describing a system that
// does not exist. A task DTO arrives with the producer that can populate one.
//
// Mirrors: ClientRemoteChangeRequest in src/core/client/command.rs, which is
// #[serde(deny_unknown_fields)] — the Rust side refuses an extra key rather
// than ignoring it, so do not add fields here without adding them there.
export interface ClientRemoteChangeRequest {
  remoteId: string;
  /**
   * The revision of the imported item the human was looking at when they
   * approved this change. Required, not optional: an opt-in staleness check is
   * not a check (§E5 contract (a)). The runtime refuses a pin that does not
   * match a revision it recorded — WORKSPACE_STALE_REVISION when the item has
   * moved, SCHEMA_INVALID when nothing imported names that remote — and posts
   * nothing in either case.
   */
  expectedRevision: string;
  title: string;
  body: string;
  /** The exact local commit the remote pull request will point at. */
  headCommit: string;
  headBranch: string;
  baseBranch: string;
}
