// Svelte 5 Reactive Client Store connecting to Tauri composition bridge (Phase A0)

import type {
  ClientCommand,
  ClientEvent,
  ClientSnapshot,
  ClientUpdate,
  ContextTaggedUpdate,
  ClientProjectSummary,
  ClientDirectoryPage,
  ClientFileOpen,
  ClientEditorPreferences,
  ClientRepositoryHistory,
  ClientOnboardingDraft,
  ClientOnboardingPreview,
  ClientWorkspaceSearchFilter,
  ClientWorkspaceSearchPage
} from './contract';
import { NO_PROJECT_REPOSITORY, NO_REVIEW_THREADS } from './contract';

export const initialSnapshot: ClientSnapshot = {
  revision: 0,
  policy: 'ask',
  runActive: false,
  usage: { inputTokens: 0, outputTokens: 0 },
  budget: { providerTurns: 0, maxProviderTurns: 25, toolCalls: 0, maxToolCalls: 100 },
  messages: [],
  messagesOmitted: 0,
  recovery: { state: 'clean' },
  models: [],
  personas: [],
  souls: [],
  routes: [],
  accounts: [],
  sessions: [],
  repository: NO_PROJECT_REPOSITORY,
  reviewThreads: NO_REVIEW_THREADS
};

export function cloneSnapshot(snapshot: ClientSnapshot = initialSnapshot): ClientSnapshot {
  return structuredClone(snapshot);
}

/**
 * A refused command, as the frontend receives it.
 *
 * `code` is the stable reason code when the runtime supplied one (AGENTS.md §6)
 * and `null` when it did not — a closed runtime has no refusal code, and the
 * frontend must not invent one to make a display simpler.
 */
export interface ClientRefusal {
  code: string | null;
  message: string;
}

/**
 * Normalize an `invoke` rejection into a typed refusal.
 *
 * Tauri rejects with the *serialized* `DesktopBridgeError`, not an `Error`, so
 * a plain `err.message` read is `undefined` and `String(err)` is
 * `"[object Object]"`. Every shape the backend can produce is handled here so
 * no refusal reaches a surface as that string.
 */
export function describeRefusal(raw: unknown): ClientRefusal {
  if (typeof raw === 'string') {
    return { code: null, message: raw };
  }
  if (raw && typeof raw === 'object') {
    const outer = raw as Record<string, unknown>;
    const payload = outer.detail;
    // `Refused { code, message }` — the adjacently-tagged struct variant.
    if (payload && typeof payload === 'object') {
      const inner = payload as Record<string, unknown>;
      if (typeof inner.message === 'string') {
        return {
          code: typeof inner.code === 'string' ? inner.code : null,
          message: inner.message
        };
      }
    }
    // `Initialization(String)` — a newtype variant carrying bare prose.
    if (typeof payload === 'string') {
      return { code: null, message: payload };
    }
    // A real `Error` (a thrown import failure, not a backend refusal).
    if (typeof outer.message === 'string') {
      return { code: null, message: outer.message };
    }
    if (typeof outer.type === 'string') {
      return { code: null, message: outer.type };
    }
  }
  return { code: null, message: String(raw) };
}

/**
 * Tell a search refusal apart from a search page.
 *
 * Discriminated on `items` rather than on `message`, because a page is the
 * shape with the field that must be present: a refusal could in principle grow
 * fields, and a page without `items` is not a page.
 */
export function isSearchRefusal(
  result: import('./contract').ClientWorkspaceSearchPage | ClientRefusal
): result is ClientRefusal {
  return !Array.isArray((result as { items?: unknown }).items);
}

/**
 * One row in the desktop fleet roster (E2) — a direct port of `FleetAgent` /
 * `apply_fleet_activity` in `src/tui/reducer.rs`, not new design. Reduced
 * client-side from `ClientEvent::SubagentActivity` because the concept is
 * TUI-local presentation state there too (never touches `RuntimeSnapshot`),
 * so the desktop client mirrors the same reduction rather than the runtime
 * growing new authority state to serve it.
 */
export interface FleetAgent {
  child: string;
  short: string;
  latest: string;
  feed: string[];
  done: boolean;
}

function shortId(child: string): string {
  return child.slice(0, 8);
}

/**
 * One row in the desktop Worktrees sidebar (E2).
 *
 * There is no durable child registry to read this from: `ChildSpec` in
 * `src/runtime/subagent/mod.rs` is transient (consumed by the orchestration
 * task, never stored on `RuntimeSnapshot`), and the D2 client-command family
 * (`CreateWorktree`, `StartChild`, …) is a stubbed refusal today
 * (`Actor::handle_child_run_command`) — there is nothing to query on
 * reconnect. `SubagentSpawned` is the only real, live signal, so — like
 * Fleet — this is reduced client-side rather than the runtime growing new
 * authority state to serve a sidebar list. Unlike Fleet, entries are never
 * cleared on a fresh convocation: the worktree still exists on disk after its
 * child settles, so clearing it here would misrepresent something real as
 * gone. `MAX_WORKTREES` bounds an otherwise-unbounded accumulation over a
 * long session, the same shape as `activityFeed`'s bound.
 */
export interface WorktreeEntry {
  child: string;
  branch: string;
  path: string;
  directive: string;
  done: boolean;
}

const MAX_WORKTREES = 50;

export class MjolnrClient {
  snapshot = $state<ClientSnapshot>(cloneSnapshot());
  activityFeed = $state<ClientEvent[]>([]);
  fleet = $state<FleetAgent[]>([]);
  worktrees = $state<WorktreeEntry[]>([]);
  resyncCount = $state<number>(0);
  connected = $state<boolean>(false);
  lastError = $state<string | null>(null);
  streamingText = $state<string>('');
  projects = $state<ClientProjectSummary[]>([]);
  selectedContextId = $state<string | null>(null);

  private addWorktree(entry: WorktreeEntry) {
    this.worktrees = [...this.worktrees.slice(-(MAX_WORKTREES - 1)), entry];
  }

  private markWorktreeDone(child: string, label: string) {
    if (label !== 'finished' && !label.startsWith('failed')) return;
    this.worktrees = this.worktrees.map((entry) =>
      entry.child === child ? { ...entry, done: true } : entry
    );
  }

  /**
   * Fold one child's forwarded activity into the fleet roster. A new
   * convocation (activity arriving when every known agent has finished)
   * clears the previous roster first, so the rail reflects the run at hand —
   * same rule as `apply_fleet_activity`.
   */
  private applyFleetActivity(child: string, label: string) {
    const done = label === 'finished' || label.startsWith('failed');
    if (this.fleet.length > 0 && this.fleet.every((agent) => agent.done)) {
      this.fleet = [];
    }
    const existing = this.fleet.find((agent) => agent.child === child);
    if (existing) {
      this.fleet = this.fleet.map((agent) =>
        agent === existing
          ? { ...agent, latest: label, feed: [...agent.feed, label], done: agent.done || done }
          : agent
      );
    } else {
      this.fleet = [...this.fleet, { child, short: shortId(child), latest: label, feed: [label], done }];
    }
  }

  constructor() {
    this.init();
  }

  async init() {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        const { listen } = await import('@tauri-apps/api/event');

        // Fetch initial snapshot
        const snap = await invoke<ClientSnapshot>('get_snapshot');
        this.snapshot = snap;
        await this.refreshProjects(invoke);
        this.connected = true;

        // Listen for updates from backend channel
        await listen<ContextTaggedUpdate>('mjolnr-update', (event) => {
          if (this.selectedContextId && event.payload.contextId !== this.selectedContextId) return;
          this.selectedContextId = event.payload.contextId;
          this.handleUpdate(event.payload.update);
        });

        // Trigger subscription channel setup
        await invoke('subscribe_updates');
      } catch (err: any) {
        this.lastError = err?.message || String(err);
        this.connected = false;
      }
    } else {
      // Browser environment outside Tauri: report disconnected without manufactured authority
      this.connected = false;
      this.lastError = 'Tauri IPC unavailable (browser mode)';
    }
  }

  private async refreshProjects(invoke: typeof import('@tauri-apps/api/core').invoke) {
    this.projects = await invoke<ClientProjectSummary[]>('list_projects');
    this.selectedContextId = this.projects.find((project) => project.selected)?.contextId ?? null;
    this.snapshot = await invoke<ClientSnapshot>('get_snapshot');
  }

  handleUpdate(update: ClientUpdate) {
    switch (update.type) {
      case 'snapshot':
        this.snapshot = update.snapshot;
        this.streamingText = '';
        break;
      case 'event':
        this.activityFeed = [...this.activityFeed.slice(-99), update.event];
        if (update.event.activity === 'subagentActivity') {
          this.applyFleetActivity(update.event.child, update.event.label);
          this.markWorktreeDone(update.event.child, update.event.label);
        }
        if (update.event.activity === 'subagentSpawned') {
          this.addWorktree({
            child: update.event.child,
            branch: update.event.branch,
            path: update.event.worktree,
            directive: update.event.directive,
            done: false
          });
        }
        if (update.event.activity === 'textDelta') {
          this.streamingText += update.event.text;
        } else if (
          update.event.activity === 'runFinished' ||
          update.event.activity === 'runFailed' ||
          update.event.activity === 'sessionEnded'
        ) {
          this.streamingText = '';
        }
        break;
      case 'resync':
        this.resyncCount += update.missed;
        this.snapshot = update.snapshot;
        this.streamingText = '';
        break;
      case 'closed':
        this.connected = false;
        this.streamingText = '';
        break;
    }
  }

  /**
   * Send a command and report the outcome.
   *
   * Returns `null` when the runtime accepted the command and a
   * [`ClientRefusal`] when it did not. Callers that have somewhere better than
   * the global error alert to show a refusal — a field, a dialog — read the
   * return value; a refusal is still set so the Attention surface keeps
   * seeing the failure. A later accepted command clears that transient bridge
   * error; durable runtime failures continue to arrive through the snapshot.
   */
  async dispatch(command: ClientCommand): Promise<ClientRefusal | null> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('dispatch_command', { command });
        if (command.type === 'openProject') await this.refreshProjects(invoke);
        this.lastError = null;
        return null;
      } catch (err: unknown) {
        const refusal = describeRefusal(err);
        this.lastError = refusal.message;
        return refusal;
      }
    }
    const refusal: ClientRefusal = {
      code: null,
      message: 'Cannot dispatch command: Tauri IPC unavailable (browser mode)'
    };
    this.lastError = refusal.message;
    return refusal;
  }

  async selectProject(contextId: string): Promise<ClientRefusal | null> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('select_project', { contextId });
        await this.refreshProjects(invoke);
        this.selectedContextId = contextId;
        return null;
      } catch (err: unknown) {
        const refusal = describeRefusal(err);
        this.lastError = refusal.message;
        return refusal;
      }
    }
    const refusal = { code: null, message: 'Cannot select project: Tauri IPC unavailable (browser mode)' };
    this.lastError = refusal.message;
    return refusal;
  }

  /**
   * One page of deterministic workspace search (Phase D4).
   *
   * Not a `dispatch`: search is a question, not a command, and it answers with
   * a page rather than by moving the snapshot. It therefore does **not** set
   * `lastError` — a query too short for the trigram index is an ordinary
   * refusal the search field explains in place, and routing it to the global
   * Attention alert would turn typing into an error state.
   *
   * Returns the page, or a [`ClientRefusal`] describing why the question could
   * not be answered. An empty page and a refusal are different answers and
   * stay different here: `{ items: [] }` means nothing matched, a refusal means
   * nothing could have.
   */
  async searchWorkspace(
    filter: ClientWorkspaceSearchFilter
  ): Promise<ClientWorkspaceSearchPage | ClientRefusal> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        return await invoke<ClientWorkspaceSearchPage>('search_workspace', { filter });
      } catch (err: unknown) {
        return describeRefusal(err);
      }
    }
    return {
      code: null,
      message: 'Cannot search: Tauri IPC unavailable (browser mode)'
    };
  }

  async listDirectory(path: string, page = 0): Promise<ClientDirectoryPage | ClientRefusal> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        return await invoke<ClientDirectoryPage>('list_directory', { path, page });
      } catch (err: unknown) {
        return describeRefusal(err);
      }
    }
    return {
      code: null,
      message: 'Cannot list files: Tauri IPC unavailable (browser mode)'
    };
  }

  async openFile(path: string): Promise<ClientFileOpen | ClientRefusal> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        return await invoke<ClientFileOpen>('open_file', { path });
      } catch (err: unknown) {
        return describeRefusal(err);
      }
    }
    return {
      code: null,
      message: 'Cannot open file: Tauri IPC unavailable (browser mode)'
    };
  }

  async loadEditorPreferences(): Promise<ClientEditorPreferences | ClientRefusal> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        return await invoke<ClientEditorPreferences>('editor_preferences_load');
      } catch (err: unknown) {
        return describeRefusal(err);
      }
    }
    return {
      code: null,
      message: 'Cannot load editor preferences: Tauri IPC unavailable (browser mode)'
    };
  }

  async saveEditorPreferences(
    preferences: ClientEditorPreferences
  ): Promise<ClientRefusal | null> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('editor_preferences_save', { preferences });
        return null;
      } catch (err: unknown) {
        return describeRefusal(err);
      }
    }
    return {
      code: null,
      message: 'Cannot save editor preferences: Tauri IPC unavailable (browser mode)'
    };
  }

  async onboardingPreview(
    draft: ClientOnboardingDraft
  ): Promise<ClientOnboardingPreview | ClientRefusal> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        return await invoke<ClientOnboardingPreview>('onboarding_preview', { draft });
      } catch (err: unknown) {
        return describeRefusal(err);
      }
    }
    return {
      code: null,
      message: 'Cannot preview onboarding files: Tauri IPC unavailable (browser mode)'
    };
  }

  async onboardingWrite(
    draft: ClientOnboardingDraft
  ): Promise<ClientOnboardingPreview | ClientRefusal> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        return await invoke<ClientOnboardingPreview>('onboarding_write', { draft });
      } catch (err: unknown) {
        const refusal = describeRefusal(err);
        this.lastError = refusal.message;
        return refusal;
      }
    }
    return {
      code: null,
      message: 'Cannot write onboarding files: Tauri IPC unavailable (browser mode)'
    };
  }

  async queryGraph(
    query: import('./contract').ClientGraphQuery
  ): Promise<import('./contract').ClientGraphPage | ClientRefusal> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        return await invoke<import('./contract').ClientGraphPage>('query_graph', { query });
      } catch (err: unknown) {
        return describeRefusal(err);
      }
    }
    return {
      code: null,
      message: 'Cannot query code graph: Tauri IPC unavailable (browser mode)'
    };
  }

  async queryBoard(): Promise<import('./contract').ClientBoardOverview | ClientRefusal> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        return await invoke<import('./contract').ClientBoardOverview>('query_board');
      } catch (err: unknown) {
        return describeRefusal(err);
      }
    }
    return {
      code: null,
      message: 'Cannot query board: Tauri IPC unavailable (browser mode)'
    };
  }

  async queryRepositoryHistory(limit = 20): Promise<ClientRepositoryHistory | ClientRefusal> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        return await invoke<ClientRepositoryHistory>('query_repository_history', { limit });
      } catch (err: unknown) {
        return describeRefusal(err);
      }
    }
    return {
      code: null,
      message: 'Cannot query repository history: Tauri IPC unavailable (browser mode)'
    };
  }

  async saveFile(
    path: string,
    expectedDigest: string,
    text: string
  ): Promise<ClientRefusal | null> {
    return this.dispatch({ type: 'saveFile', path, expectedDigest, text });
  }

  async resumeSession(session: string) {
    return this.dispatch({ type: 'resumeSession', session });
  }

  async resolveResume(choice: import('./contract').ClientResumeChoice) {
    return this.dispatch({ type: 'resolveResume', choice });
  }

  async createSession(provider: string, model: string) {
    return this.dispatch({ type: 'createSession', provider, model });
  }

  async sendMessage(text: string) {
    return this.dispatch({ type: 'sendMessage', text });
  }

  async cancelRun() {
    return this.dispatch({ type: 'cancelRun' });
  }

  async resolveApproval(approval: string, decision: import('./contract').ClientApprovalDecision) {
    return this.dispatch({ type: 'resolveApproval', approval, decision });
  }

  async resolveRecovery(decision: import('./contract').ClientRecoveryDecision) {
    return this.dispatch({ type: 'resolveRecovery', decision });
  }

  async setPolicy(policy: import('./contract').ClientPolicy) {
    return this.dispatch({ type: 'setPolicy', policy });
  }

  async endSession() {
    return this.dispatch({ type: 'endSession' });
  }

  /**
   * Leave the open session without ending it.
   *
   * The runtime allows one open session at a time. `endSession` is terminal —
   * an ended session can never be resumed — so switching sessions goes through
   * here, which only drops the lease.
   */
  async releaseSession() {
    return this.dispatch({ type: 'releaseSession' });
  }

  /**
   * Break the write lease a crashed mjolnr left on a session.
   *
   * The runtime never does this on its own — it cannot prove the holder is
   * gone — so this carries a human's assertion that it is.
   */
  async reclaimSession(session: string) {
    return this.dispatch({ type: 'reclaimSession', session });
  }

  /** Authenticate LM Studio: save endpoint + optional token, then refresh. */
  async authLmStudioLogin(address: string, token: string): Promise<{ endpoint: string } | { error: string }> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        const endpoint = await invoke<string>('auth_lm_studio_login', { address, token });
        await this.dispatch({ type: 'requestSnapshot' });
        return { endpoint };
      } catch (err: unknown) {
        const refusal = describeRefusal(err);
        this.lastError = refusal.message;
        return { error: refusal.message };
      }
    }
    const msg = 'Tauri IPC unavailable (browser mode)';
    this.lastError = msg;
    return { error: msg };
  }

  /** Store an API key for a provider, then refresh. */
  async authApiKeyLogin(provider: string, key: string): Promise<true | { error: string }> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('auth_api_key_login', { provider, key });
        await this.dispatch({ type: 'requestSnapshot' });
        return true;
      } catch (err: unknown) {
        const refusal = describeRefusal(err);
        this.lastError = refusal.message;
        return { error: refusal.message };
      }
    }
    const msg = 'Tauri IPC unavailable (browser mode)';
    this.lastError = msg;
    return { error: msg };
  }

  /** Verify Jules against its source catalog before storing its key. */
  async authJulesLogin(key: string): Promise<true | { error: string }> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('auth_jules_login', { key });
        return true;
      } catch (err: unknown) {
        const refusal = describeRefusal(err);
        this.lastError = refusal.message;
        return { error: refusal.message };
      }
    }
    const msg = 'Tauri IPC unavailable (browser mode)';
    this.lastError = msg;
    return { error: msg };
  }

  /** Start the desktop loopback OAuth flow for Gemini CLI or Antigravity. */
  async authGoogleOAuth(provider: string): Promise<true | { error: string }> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('auth_google_oauth', { provider });
        await this.dispatch({ type: 'requestSnapshot' });
        return true;
      } catch (err: unknown) {
        const refusal = describeRefusal(err);
        this.lastError = refusal.message;
        return { error: refusal.message };
      }
    }
    const msg = 'Tauri IPC unavailable (browser mode)';
    this.lastError = msg;
    return { error: msg };
  }

  /** Start the Codex subscription device flow, then refresh discovered models. */
  async authCodexOAuth(): Promise<true | { error: string }> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('auth_codex_oauth');
        await this.dispatch({ type: 'requestSnapshot' });
        return true;
      } catch (err: unknown) {
        const refusal = describeRefusal(err);
        this.lastError = refusal.message;
        return { error: refusal.message };
      }
    }
    const msg = 'Tauri IPC unavailable (browser mode)';
    this.lastError = msg;
    return { error: msg };
  }

  /** Start Claude's paste-code flow. Completion is submitted separately. */
  async authAnthropicOAuthStart(): Promise<true | { error: string }> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('auth_anthropic_oauth_start');
        return true;
      } catch (err: unknown) {
        const refusal = describeRefusal(err);
        this.lastError = refusal.message;
        return { error: refusal.message };
      }
    }
    const msg = 'Tauri IPC unavailable (browser mode)';
    this.lastError = msg;
    return { error: msg };
  }

  async authAnthropicOAuthComplete(code: string): Promise<true | { error: string }> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('auth_anthropic_oauth_complete', { code });
        await this.dispatch({ type: 'requestSnapshot' });
        return true;
      } catch (err: unknown) {
        const refusal = describeRefusal(err);
        this.lastError = refusal.message;
        return { error: refusal.message };
      }
    }
    const msg = 'Tauri IPC unavailable (browser mode)';
    this.lastError = msg;
    return { error: msg };
  }

  async authJulesStatus(): Promise<boolean> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        return await invoke<boolean>('auth_jules_status');
      } catch {
        return false;
      }
    }
    return false;
  }

  /** Remove a stored credential for a provider, then refresh. */
  async authLogout(provider: string): Promise<boolean> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('auth_logout', { provider });
        await this.dispatch({ type: 'requestSnapshot' });
        return true;
      } catch (err: unknown) {
        const refusal = describeRefusal(err);
        this.lastError = refusal.message;
        return false;
      }
    }
    this.lastError = 'Tauri IPC unavailable (browser mode)';
    return false;
  }
}

export const clientStore = new MjolnrClient();

export function resetClientStoreForTests() {
  clientStore.snapshot = cloneSnapshot();
  clientStore.activityFeed = [];
  clientStore.fleet = [];
  clientStore.worktrees = [];
  clientStore.resyncCount = 0;
  clientStore.connected = false;
  clientStore.lastError = null;
  clientStore.streamingText = '';
}
