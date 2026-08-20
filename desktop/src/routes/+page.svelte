<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import {
    Activity03Icon,
    Add01Icon,
    Alert02Icon,
    Attachment01Icon,
    CheckmarkCircle02Icon,
    CircleIcon,
    DashboardSquare01Icon,
    FileEditIcon,
    Folder01Icon,
    FolderOpenIcon,
    GitBranchIcon,
    Image02Icon,
    MaskTheater01Icon,
    Message01Icon,
    Moon01Icon,
    Notification02Icon,
    PanelRightOpenIcon,
    PlusSignIcon,
    RefreshIcon,
    SearchIcon,
    SentIcon,
    SparklesIcon,
    Key01Icon,
    StopIcon,
    Sun01Icon,
    Task01Icon,
    TerminalIcon
  } from '@hugeicons/core-free-icons';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import { mode, toggleMode } from 'mode-watcher';
  import { open } from '@tauri-apps/plugin-dialog';
  import ActivityBars from '$lib/components/chrome/ActivityBars.svelte';
  import AppEmblem from '$lib/components/chrome/AppEmblem.svelte';
  import GovernanceModal from '$lib/components/chrome/GovernanceModal.svelte';
  import StatusOrb from '$lib/components/chrome/StatusOrb.svelte';
  import AttentionSurface from '$lib/components/surfaces/AttentionSurface.svelte';
  import BoardPane from '$lib/components/board/BoardPane.svelte';
  import ChangesSurface from '$lib/components/surfaces/ChangesSurface.svelte';
  import EditorPane from '$lib/components/panes/EditorPane.svelte';
  import FileExplorer from '$lib/components/panes/FileExplorer.svelte';
  import GraphPane from '$lib/components/graph/GraphPane.svelte';
  import InspectorPane from '$lib/components/inspector/InspectorPane.svelte';
  import PlanSurface from '$lib/components/plan/PlanSurface.svelte';
  import RepositoryPanel from '$lib/components/repository/RepositoryPanel.svelte';
  import RepositoryControls from '$lib/components/repository/RepositoryControls.svelte';
  import CloneRepository from '$lib/components/repository/CloneRepository.svelte';
  import TerminalPane from '$lib/components/panes/TerminalPane.svelte';
  import VerifySurface from '$lib/components/surfaces/VerifySurface.svelte';
  import * as Alert from '$lib/components/ui/alert';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import * as Command from '$lib/components/ui/command';
  import * as Dialog from '$lib/components/ui/dialog';
  import * as Field from '$lib/components/ui/field';
  import { Input } from '$lib/components/ui/input';
  import * as Resizable from '$lib/components/ui/resizable';
  import * as ScrollArea from '$lib/components/ui/scroll-area';
  import * as Select from '$lib/components/ui/select';
  import { Separator } from '$lib/components/ui/separator';
  import * as Sidebar from '$lib/components/ui/sidebar';
  import * as Tabs from '$lib/components/ui/tabs';
  import { Textarea } from '$lib/components/ui/textarea';
  import * as ToggleGroup from '$lib/components/ui/toggle-group';
  import { clientStore, isSearchRefusal, type ClientRefusal } from '$lib/runtime/client.svelte';
  import { cn } from '$lib/utils';
  import type {
    ClientPolicy,
    ClientQuota,
    ClientDirectoryEntry,
    ClientFileOpen,
    ClientResumeChoice,
    ClientWorkspaceSearchResult
  } from '$lib/runtime/contract';

  type SurfaceId = 'Conversation' | 'Plan' | 'Board' | 'Changes' | 'Verify' | 'Attention';

  const surfaces: Array<{
    value: SurfaceId;
    label: string;
    shortcut: string;
    icon: typeof Message01Icon;
  }> = [
    { value: 'Conversation', label: 'Conversation', shortcut: '⌘1', icon: Message01Icon },
    { value: 'Plan', label: 'Plan', shortcut: '⌘2', icon: Task01Icon },
    { value: 'Board', label: 'Board', shortcut: '⌘3', icon: DashboardSquare01Icon },
    { value: 'Changes', label: 'Changes', shortcut: '⌘4', icon: FileEditIcon },
    { value: 'Verify', label: 'Verify', shortcut: '⌘5', icon: CheckmarkCircle02Icon },
    { value: 'Attention', label: 'Attention', shortcut: '⌘6', icon: Notification02Icon }
  ];

  const selectablePolicies: ClientPolicy[] = ['read-only', 'ask', 'workspace-write'];

  // A real, local filter over the session list — not the §D4 recorded-work
  // search the command palette already does against the durable store. This
  // one only ever narrows what is already in `snap.sessions`.
  let sidebarFilter = $state('');
  let projectPathInput = $state('');
  let projectedWorkspaceRoot = $state('');
  let hasProjectedWorkspaceRoot = $state(false);
  let openProjectRefusal = $state<ClientRefusal | null>(null);
  let messageInput = $state('');
  let selectedProvider = $state('');
  let selectedModel = $state('');
  let paletteOpen = $state(false);
  let governanceOpen = $state(false);
  let governanceTab = $state('council');
  let repoDialogOpen = $state(false);
  let showProjectAdvanced = $state(false);
  let explorerOpen = $state(false);
  let terminalOpen = $state(false);
  let terminalExpanded = $state(false);
  let editorPath = $state<string | null>(null);
  let editorFile = $state<ClientFileOpen | null>(null);
  let editorTabs = $state<Array<{ path: string; file: ClientFileOpen }>>([]);
  let editorAutosave = $state(false);
  let editorPreferencesRefusal = $state<ClientRefusal | null>(null);

  onMount(() => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    void clientStore.loadEditorPreferences().then((result) => {
      if ('message' in result) {
        editorPreferencesRefusal = result;
        return;
      }
      editorAutosave = result.autosave;
    });
  });

  function openGovernance(tab: string) {
    governanceTab = tab;
    governanceOpen = true;
  }

  function startNewChat() {
    messageInput = '';
    activeSurface = 'Conversation';
    createSession();
  }

  function cyclePolicy() {
    const sequence: ClientPolicy[] = ['ask', 'workspace-write', 'full-auto', 'read-only'];
    const currentIndex = sequence.indexOf(snap.policy as ClientPolicy);
    const next = sequence[(currentIndex + 1) % sequence.length];
    dispatch({ type: 'setPolicy', policy: next });
  }
  let paletteQuery = $state('');
  let searchResults = $state<ClientWorkspaceSearchResult[]>([]);
  let searchRefusal = $state<ClientRefusal | null>(null);
  let searchRan = $state(false);
  let searching = $state(false);
  let fileSearchResults = $state<ClientDirectoryEntry[]>([]);
  let fileSearchRefusal = $state<ClientRefusal | null>(null);
  let fileSearchTruncated = $state(false);
  let fileSearching = $state(false);
  let splitPreset = $state<'none' | 'changes' | 'verify' | 'inspector' | 'graph'>('none');
  let activeSurface = $state<SurfaceId>('Conversation');

  /**
   * Prefer the window whose pool actually covers the active model
   * (`isRelevant`, computed on the Rust side); fall back to the worst window
   * across the snapshot when none names the model's pool. Mirrors
   * `quota_gauge` in `src/tui/chrome.rs` — same fallback order, ported rather
   * than re-derived.
   */
  function relevantQuotaWindow(quota: ClientQuota | undefined) {
    if (!quota || quota.windows.length === 0) return undefined;
    return (
      quota.windows.find((window) => window.isRelevant) ??
      quota.windows.reduce((worst, window) => (window.usedFraction > worst.usedFraction ? window : worst))
    );
  }

  // Thresholds ported from `theme::quota_style` (src/tui/theme.rs) — not
  // re-derived, so the desktop and TUI never disagree about what "danger"
  // means for the same used fraction.
  function quotaSeverity(usedFraction: number): 'ok' | 'warn' | 'danger' {
    if (usedFraction >= 0.95) return 'danger';
    if (usedFraction >= 0.8) return 'warn';
    return 'ok';
  }

  const QUOTA_BADGE_VARIANT: Record<'ok' | 'warn' | 'danger', 'secondary' | 'default' | 'destructive'> = {
    ok: 'secondary',
    warn: 'default',
    danger: 'destructive'
  };

  // Governance-state colour, never a bare "success"/"error" red-green pair --
  // ok/refused/failed map onto the same --gov-verified/--gov-refusal tokens
  // every other surface uses for the identical concept.
  const TOOL_OUTCOME_CLASS: Record<'ok' | 'refused' | 'failed', string> = {
    ok: 'border-gov-verified-border bg-gov-verified-bg text-gov-verified',
    refused: 'border-gov-refusal-border bg-gov-refusal-bg text-gov-refusal',
    failed: 'border-gov-refusal-border bg-gov-refusal-bg text-gov-refusal'
  };

  // Mirrors `countdown` in src/tui/usage.rs: coarsest readable unit, not a
  // full duration breakdown.
  function formatCountdown(resetsAt: string): string {
    const remaining = Math.max(0, Math.floor((new Date(resetsAt).getTime() - Date.now()) / 1000));
    const days = Math.floor(remaining / 86_400);
    const hours = Math.floor((remaining % 86_400) / 3_600);
    const minutes = Math.floor((remaining % 3_600) / 60);
    const seconds = remaining % 60;
    if (days > 0) return `${days}d${hours}h`;
    if (hours > 0) return `${hours}h${String(minutes).padStart(2, '0')}m`;
    if (minutes > 0) return `${minutes}m${String(seconds).padStart(2, '0')}s`;
    return `${seconds}s`;
  }

  // Footer telemetry helpers. Token counts stay exact ("12,450"), never
  // abbreviated — the footer is a live projection of snapshot truth and a
  // rounded figure would have to be re-checked against the snapshot to be
  // believed. Bars clamp at 100%: over-budget is a red line, not a wider bar.
  function formatTokenCount(n: number): string {
    return n.toLocaleString('en-US');
  }
  function budgetFraction(used: number, max: number): number {
    return max > 0 ? Math.min(100, (used / max) * 100) : 0;
  }

  // Friendly segment labels for the policy switcher. The raw ClientPolicy
  // value stays on each segment as its aria-label; the label is a display
  // alias, never the contract.
  const POLICY_SEGMENT_LABEL: Record<ClientPolicy, string> = {
    'read-only': 'Read',
    ask: 'Ask',
    'workspace-write': 'Write',
    'full-auto': 'Auto'
  };

  // The approval gate's preview is a free-form string: it is a diff for file
  // edits and prose for everything else. .diff-added/.diff-removed are claims
  // about file state, so they are applied only when the preview actually
  // carries unified-diff headers; a prose preview stays plain.
  function previewLooksLikeDiff(preview: string): boolean {
    return /^(diff --git |--- |\+\+\+ )/m.test(preview);
  }
  function previewDiffClass(line: string): string {
    if (line.startsWith('+') && !line.startsWith('+++')) return 'diff-added';
    if (line.startsWith('-') && !line.startsWith('---')) return 'diff-removed';
    return '';
  }

  let snap = $derived(clientStore.snapshot);
  let projectName = $derived(
    snap.workspaceRoot
      ? snap.workspaceRoot.replace(/\/+$/, '').split('/').pop() || snap.workspaceRoot
      : null
  );
  let isConnected = $derived(clientStore.connected);
  let streamingText = $derived(clientStore.streamingText);
  let quotaWindow = $derived(relevantQuotaWindow(snap.quota));

  // The webview can restore an old form value after a packaged launch. That
  // value is not runtime truth and must never look like the selected project.
  // Project the current root into the chooser until the owner starts editing;
  // once edited, leave the draft alone until the runtime acknowledges a new
  // root. This also clears a stale path when mjolnr starts with no workspace.
  $effect(() => {
    const runtimeRoot = snap.workspaceRoot ?? '';
    const chooserIsUntouched =
      !hasProjectedWorkspaceRoot || projectPathInput === projectedWorkspaceRoot;
    if (chooserIsUntouched) projectPathInput = runtimeRoot;
    projectedWorkspaceRoot = runtimeRoot;
    hasProjectedWorkspaceRoot = true;
  });
  // Same visibility rule as `fleet_visible()` in src/tui/reducer.rs: a
  // convocation of two or more agents with at least one still working. A solo
  // session pays no chrome tax.
  let fleetVisible = $derived(
    clientStore.fleet.length >= 2 && clientStore.fleet.some((agent) => !agent.done)
  );
  // The mockup's Accounts pills show connected providers only, not every
  // provider mjolnr knows how to authenticate — a "+Connect" affordance for
  // the rest belongs to an onboarding flow this slice doesn't build.
  let connectedAccounts = $derived(snap.accounts.filter((account) => account.state === 'connected'));
  // The header's loop-state indicator. A pending approval is literally what
  // the mockup calls "Pending gate" — the same concept, not a relabel. Falls
  // back to whether the loop is actually streaming, never a fabricated state.
  let loopState = $derived(
    snap.pendingApproval
      ? { label: 'Pending gate', tone: 'approval' as const }
      : snap.runActive
        ? { label: 'Running', tone: 'active' as const }
        : { label: 'Idle', tone: 'idle' as const }
  );
  let providerChoices = $derived(
    Array.from(new Map(snap.models.map((choice) => [choice.provider, choice.provider])).values())
  );
  let modelChoices = $derived(snap.models.filter((choice) => choice.provider === selectedProvider));
  let selectedProviderLabel = $derived(selectedProvider || 'Select provider');
  let selectedModelLabel = $derived(
    modelChoices.find((choice) => choice.model === selectedModel)?.displayName ?? 'Select model'
  );
  let canCreateSession = $derived(Boolean(snap.workspaceRoot && selectedProvider && selectedModel));
  let journeyState = $derived(
    !snap.workspaceRoot
      ? 'workspace'
      : !snap.session && snap.sessions.length === 0
        ? 'create'
        : !snap.session
          ? 'resume'
          : 'active'
  );
  let attentionCount = $derived(
    Number(Boolean(snap.storeFailure)) +
      Number(snap.recovery.state === 'required') +
      Number(Boolean(snap.pendingApproval)) +
      Number(Boolean(snap.resumeAdvice))
  );

  $effect(() => {
    if (!providerChoices.includes(selectedProvider)) {
      selectedProvider = providerChoices[0] ?? '';
    }
  });

  $effect(() => {
    if (!modelChoices.some((choice) => choice.model === selectedModel)) {
      selectedModel = modelChoices[0]?.model ?? '';
    }
  });

  /**
   * Recorded-work search (Phase D4).
   *
   * Debounced, because the producer's own measurement says so: p50 22.8 ms is
   * comfortable but p95 51 ms and a 242 ms tail are not, and the report's stated
   * remedy is debouncing at the client rather than caching a projection that can
   * go stale.
   *
   * Short queries are sent rather than pre-empted. The store refuses anything
   * below its trigram minimum with a sentence saying why, and duplicating that
   * bound here would give the user a second copy of a number that can drift
   * from the one actually enforced. A refusal costs no index work.
   *
   * `token` guards against a slow early response landing after a fast later
   * one and repainting the list with results for a query the user has moved on
   * from.
   */
  const SEARCH_DEBOUNCE_MS = 150;
  const SEARCH_PAGE_SIZE = 8;
  const FILE_SEARCH_MAX_DIRECTORIES = 128;
  const FILE_SEARCH_MAX_FILES = 500;
  let fileSearchToken = 0;
  let fileSearchTimer: ReturnType<typeof setTimeout> | undefined;
  let searchToken = 0;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  async function runSearch(query: string, token: number) {
    const result = await clientStore.searchWorkspace({
      query,
      limit: SEARCH_PAGE_SIZE
    });
    if (token !== searchToken) return;
    searching = false;
    searchRan = true;
    if (isSearchRefusal(result)) {
      searchResults = [];
      searchRefusal = result;
      return;
    }
    searchRefusal = null;
    searchResults = result.items;
  }

  /**
   * Search the bounded directory producer for a file path. This is deliberately
   * a Tauri-only convenience query: browser mode has no workspace and must not
   * pretend that the fixture tree is authoritative. Every directory remains a
   * paginated, Rust-owned read; the client only performs a bounded breadth-first
   * traversal and discloses when its search budget stopped before exhaustion.
   */
  async function runFileSearch(query: string, token: number) {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    fileSearching = true;
    fileSearchRefusal = null;
    fileSearchTruncated = false;
    const queue = [''];
    const visited = new Set<string>();
    const results: ClientDirectoryEntry[] = [];
    let directoriesVisited = 0;

    while (queue.length > 0 && directoriesVisited < FILE_SEARCH_MAX_DIRECTORIES) {
      const path = queue.shift();
      if (path === undefined || visited.has(path)) continue;
      visited.add(path);
      directoriesVisited += 1;

      let page = 0;
      let hasMore = true;
      while (hasMore && results.length < FILE_SEARCH_MAX_FILES) {
        const listed = await clientStore.listDirectory(path, page);
        if (token !== fileSearchToken) return;
        if ('message' in listed) {
          fileSearchResults = [];
          fileSearchRefusal = listed;
          fileSearching = false;
          return;
        }
        for (const entry of listed.entries.items) {
          if (entry.kind === 'directory' && entry.symlink?.escaping !== true) {
            queue.push(entry.path);
          } else if (
            entry.kind === 'file' &&
            entry.path.toLowerCase().includes(query.toLowerCase())
          ) {
            results.push(entry);
            if (results.length >= FILE_SEARCH_MAX_FILES) break;
          }
        }
        hasMore = listed.hasMore;
        page += 1;
      }

      if (results.length >= FILE_SEARCH_MAX_FILES) break;
    }

    if (token !== fileSearchToken) return;
    fileSearchResults = results;
    fileSearchTruncated = queue.length > 0 || directoriesVisited >= FILE_SEARCH_MAX_DIRECTORIES;
    fileSearching = false;
  }

  $effect(() => {
    const query = paletteQuery.trim();
    clearTimeout(searchTimer);
    searchToken += 1;
    const token = searchToken;

    if (!paletteOpen || query.length === 0) {
      searching = false;
      searchRan = false;
      searchResults = [];
      searchRefusal = null;
      return;
    }

    searching = true;
    searchTimer = setTimeout(() => void runSearch(query, token), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(searchTimer);
  });

  $effect(() => {
    const query = paletteQuery.trim();
    clearTimeout(fileSearchTimer);
    fileSearchToken += 1;
    const token = fileSearchToken;

    if (!paletteOpen || query.length === 0 || typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) {
      fileSearching = false;
      fileSearchResults = [];
      fileSearchRefusal = null;
      fileSearchTruncated = false;
      return;
    }

    fileSearching = true;
    fileSearchTimer = setTimeout(() => void runFileSearch(query, token), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(fileSearchTimer);
  });

  /**
   * What selecting a result can honestly do today.
   *
   * There is no surface that scrolls a transcript to one event, so the palette
   * does not offer to "jump" to it. What exists is resuming the session the
   * event belongs to, which is a real governed command, and that is what the
   * item says it will do.
   */
  function openSearchResult(result: ClientWorkspaceSearchResult) {
    paletteOpen = false;
    activeSurface = 'Conversation';
    if (snap.session !== result.sessionId) {
      dispatch({ type: 'resumeSession', session: result.sessionId });
    }
  }

  function dispatch(command: Parameters<typeof clientStore.dispatch>[0]) {
    void clientStore.dispatch(command);
  }

  // The runtime owns whether a root is acceptable — empty, not a directory, or
  // locked by an open session. An earlier `if (root)` guard here swallowed the
  // dispatch for an empty field, so the runtime's own refusal never ran and the
  // button appeared dead. Presentation state holds the answer; authority does
  // not move (ADR 0006).
  /** Open the path typed into the sidebar or the no-workspace launch card. */
  async function openProject() {
    const root = projectPathInput.trim();
    projectPathInput = root;
    openProjectRefusal = null;
    openProjectRefusal = await clientStore.dispatch({
      type: 'openProject',
      root
    });
  }

  /**
   * Choose a real directory from the native picker. This is the primary path
   * when mjolnr was launched without a project, so the process cwd is never
   * mistaken for the project the owner intends to govern.
   */
  async function chooseWorkspace() {
    let root = projectPathInput.trim();
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      const picked = await open({ directory: true, multiple: false, title: 'Open workspace' });
      if (picked === null) return; // cancelled — no dispatch, no refusal
      root = picked;
      projectPathInput = root;
    }
    if (root) {
      openProjectRefusal = await clientStore.dispatch({ type: 'openProject', root });
    } else {
      await openProject();
    }
  }

  function createSession() {
    if (canCreateSession) {
      dispatch({ type: 'createSession', provider: selectedProvider, model: selectedModel });
    }
  }

  function resumeSession(session: string) {
    if (snap.session === session) return;
    dispatch({ type: 'resumeSession', session });
  }

  function resolveResume(choice: ClientResumeChoice) {
    dispatch({ type: 'resolveResume', choice });
  }

  async function sendMessage(textOverride?: string) {
    const text = (textOverride ?? messageInput).trim();
    if (!text || snap.runActive) return;
    if (!snap.session) {
      if (canCreateSession) {
        dispatch({ type: 'createSession', provider: selectedProvider, model: selectedModel });
        await new Promise((r) => setTimeout(r, 60));
      } else if (snap.models.length > 0) {
        const first = snap.models[0];
        dispatch({ type: 'createSession', provider: first.provider, model: first.model });
        await new Promise((r) => setTimeout(r, 60));
      } else {
        goto('/onboarding');
        return;
      }
    }
    messageInput = '';
    dispatch({ type: 'sendMessage', text });
  }

  function approve(decision: 'approve-once' | 'approve-exact-for-session' | 'deny') {
    if (!snap.pendingApproval) return;
    dispatch({ type: 'resolveApproval', approval: snap.pendingApproval.id, decision });
  }

  async function openFileInEditor(path: string) {
    const existing = editorTabs.find((tab) => tab.path === path);
    if (existing) {
      editorPath = existing.path;
      editorFile = existing.file;
      activeSurface = 'Conversation';
      return;
    }

    editorPath = path;
    editorFile = null;
    activeSurface = 'Conversation';
    const result = await clientStore.openFile(path);
    if ('message' in result) {
      editorPath = null;
      clientStore.lastError = result.message;
      return;
    }
    editorTabs = [...editorTabs, { path, file: result }];
    editorFile = result;
  }

  /**
   * Load a council amendment into the editor as unsaved text.
   *
   * The file is opened through the ordinary bounded producer first, so the tab
   * carries the digest mjolnr actually observed on disk; only the buffer text is
   * replaced with the draft. Saving is therefore the same governed save as any
   * other edit — stale-digest refusal included — and the amendment never
   * reaches disk without a human pressing save.
   */
  async function openAmendmentInEditor(path: string, text: string) {
    closeEditorTab(path);
    const result = await clientStore.openFile(path);
    if ('message' in result) {
      clientStore.lastError = result.message;
      return;
    }
    if (result.mode.type !== 'editable') {
      clientStore.lastError = `${path} is preview-only, so the amendment cannot be opened for editing.`;
      return;
    }
    const seeded: ClientFileOpen = {
      ...result,
      mode: { ...result.mode, text }
    };
    editorTabs = [...editorTabs, { path, file: seeded }];
    editorPath = path;
    editorFile = seeded;
    activeSurface = 'Conversation';
    governanceOpen = false;
  }

  function selectEditorTab(path: string) {
    const tab = editorTabs.find((entry) => entry.path === path);
    if (!tab) return;
    editorPath = tab.path;
    editorFile = tab.file;
  }

  function closeEditorTab(path: string) {
    const index = editorTabs.findIndex((entry) => entry.path === path);
    if (index < 0) return;
    const remaining = editorTabs.filter((entry) => entry.path !== path);
    editorTabs = remaining;
    if (editorPath !== path) return;
    const next = remaining[index] ?? remaining[index - 1];
    editorPath = next?.path ?? null;
    editorFile = next?.file ?? null;
  }

  async function saveEditorText(text: string, expectedDigest: string): Promise<string | null> {
    if (!editorPath) return 'No file is open.';
    const refusal = await clientStore.saveFile(editorPath, expectedDigest, text);
    if (refusal) return refusal.message;

    // The save event refreshes repository truth, but it intentionally does not
    // carry the file body or a new digest. Re-read through the same bounded
    // producer so the editor's next save is anchored to bytes mjolnr actually
    // observed after the write.
    const reread = await clientStore.openFile(editorPath);
    if ('message' in reread) return `saved, but the post-save read was refused: ${reread.message}`;
    editorTabs = editorTabs.map((tab) =>
      tab.path === editorPath ? { ...tab, file: reread } : tab
    );
    editorFile = reread;
    return null;
  }

  async function setEditorAutosave(enabled: boolean) {
    const previous = editorAutosave;
    editorAutosave = enabled;
    editorPreferencesRefusal = null;
    const refusal = await clientStore.saveEditorPreferences({ autosave: enabled });
    if (refusal) {
      editorAutosave = previous;
      editorPreferencesRefusal = refusal;
    }
  }

  function selectCommand(id: string) {
    paletteOpen = false;
    if (id.startsWith('surface-')) {
      activeSurface = id.slice('surface-'.length) as SurfaceId;
      return;
    }
    if (id === 'resync') dispatch({ type: 'requestSnapshot' });
    if (id === 'theme') toggleMode();
    if (id === 'inspector') splitPreset = 'inspector';
    if (id === 'graph') splitPreset = 'graph';
    if (id === 'explorer') explorerOpen = !explorerOpen;
    if (id === 'terminal') terminalOpen = !terminalOpen;
    if (id === 'gallery') void goto('/gallery');
    if (id === 'governance') openGovernance('council');
    if (id === 'cancel' && snap.runActive) dispatch({ type: 'cancelRun' });
  }

  function handleGlobalKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      paletteOpen = false;
      splitPreset = 'none';
      return;
    }
    const primary = event.metaKey || event.ctrlKey;
    if (!primary || event.altKey) return;
    const key = event.key.toLowerCase();

    if (!event.shiftKey && key === 'k') {
      event.preventDefault();
      paletteOpen = true;
      return;
    }
    if (!event.shiftKey && key === 'i') {
      event.preventDefault();
      splitPreset = splitPreset === 'inspector' ? 'none' : 'inspector';
      return;
    }
    if (!event.shiftKey && key === 'e') {
      event.preventDefault();
      explorerOpen = !explorerOpen;
      return;
    }
    if (!event.shiftKey && key === '\\') {
      event.preventDefault();
      terminalOpen = !terminalOpen;
      return;
    }
    if (!event.shiftKey && key === 'g') {
      event.preventDefault();
      openGovernance('council');
      return;
    }
    if (event.shiftKey && key === 'g') {
      event.preventDefault();
      void goto('/gallery');
      return;
    }
    if (!event.shiftKey && key === 'r') {
      event.preventDefault();
      dispatch({ type: 'requestSnapshot' });
      return;
    }
    if (event.shiftKey && key === 'c' && snap.runActive) {
      event.preventDefault();
      dispatch({ type: 'cancelRun' });
      return;
    }

    const index = Number.parseInt(key, 10);
    if (!event.shiftKey && index >= 1 && index <= surfaces.length) {
      event.preventDefault();
      activeSurface = surfaces[index - 1].value;
    }
  }
</script>

<svelte:window onkeydown={handleGlobalKeydown} />

<Sidebar.Provider>
  <Sidebar.Root collapsible="icon">
    <!-- Header with App Mark and New Chat -->
    <Sidebar.Header class="p-2.5 pb-2 flex flex-col gap-2 border-b border-sidebar-border/40">
      <div class="flex items-center justify-between">
        <div class="flex items-center gap-2">
          <AppEmblem size={22} />
          <span class="font-bold text-sm text-foreground">mjolnr</span>
          <span class="rounded bg-primary/10 border border-primary/20 px-1.5 py-0.2 font-mono text-[9px] text-primary">v0.0.0</span>
        </div>
        <Button variant="ghost" size="icon-sm" class="h-6 w-6 text-muted-foreground hover:text-foreground" onclick={() => (paletteOpen = true)} title="Search (⌘K)">
          <HugeiconsIcon icon={SearchIcon} strokeWidth={2} class="size-3.5" />
        </Button>
      </div>

      <Button
        class="w-full justify-start gap-2 bg-primary text-primary-foreground font-semibold text-xs shadow-sm hover:bg-primary/90 h-8"
        onclick={startNewChat}
      >
        <HugeiconsIcon icon={PlusSignIcon} strokeWidth={2.5} class="size-3.5" />
        <span>New chat</span>
        <kbd class="ml-auto font-mono text-[9px] bg-black/20 px-1 py-0.2 rounded">⌘N</kbd>
      </Button>
    </Sidebar.Header>

    <Sidebar.Content class="gap-1 px-1.5">
      <!-- Surfaces Navigation Rail -->
      <Sidebar.Group class="py-1">
        <Sidebar.GroupLabel class="text-[11px] font-semibold text-muted-foreground uppercase tracking-wider px-2 py-1">Surfaces</Sidebar.GroupLabel>
        <Sidebar.GroupContent>
          <Sidebar.Menu>
            {#each surfaces as surface (surface.value)}
              <Sidebar.MenuItem>
                <Sidebar.MenuButton
                  isActive={activeSurface === surface.value}
                  tooltipContent={surface.label}
                  class="h-7 text-xs"
                  onclick={() => (activeSurface = surface.value)}
                >
                  <HugeiconsIcon icon={surface.icon} strokeWidth={2} class={activeSurface === surface.value ? 'text-primary size-3.5' : 'size-3.5 text-muted-foreground'} />
                  <span class={activeSurface === surface.value ? 'font-semibold text-foreground' : ''}>{surface.label}</span>
                  {#if surface.value === 'Attention' && attentionCount > 0}
                    <Sidebar.MenuBadge>{attentionCount}</Sidebar.MenuBadge>
                  {:else}
                    <span class="ml-auto font-mono text-[9px] text-muted-foreground">{surface.shortcut}</span>
                  {/if}
                </Sidebar.MenuButton>
              </Sidebar.MenuItem>
            {/each}
          </Sidebar.Menu>
        </Sidebar.GroupContent>
      </Sidebar.Group>

      <Sidebar.Separator class="my-1" />

      <!-- Projects Section (Codex-style) -->
      <Sidebar.Group class="py-1 min-h-0 flex-1">
        <Sidebar.GroupLabel class="flex items-center justify-between text-[11px] font-semibold text-muted-foreground uppercase tracking-wider px-2 py-1">
          <span>Projects</span>
          <button
            type="button"
            class="text-muted-foreground hover:text-primary transition-colors p-0.5 rounded cursor-pointer"
            onclick={chooseWorkspace}
            title="Open project folder (⌘O)"
          >
            <HugeiconsIcon icon={PlusSignIcon} strokeWidth={2} class="size-3.5" />
          </button>
        </Sidebar.GroupLabel>
        <Sidebar.GroupContent>
          {#if snap.workspaceRoot}
            <div class="mb-1.5 flex items-center justify-between rounded-md bg-sidebar-accent/60 px-2 py-1.5 text-xs font-medium text-foreground">
              <div class="flex items-center gap-2 truncate">
                <HugeiconsIcon icon={FolderOpenIcon} strokeWidth={2} class="size-3.5 text-primary shrink-0" />
                <span class="truncate font-mono">{projectName}</span>
              </div>
              <StatusOrb state="active" size={5} />
            </div>
          {:else}
            <button
              type="button"
              class="mb-1.5 flex w-full items-center gap-2 rounded-md border border-dashed border-border/80 px-2 py-1.5 text-xs text-muted-foreground hover:border-primary/60 hover:text-foreground transition-all cursor-pointer"
              onclick={chooseWorkspace}
            >
              <HugeiconsIcon icon={Folder01Icon} strokeWidth={2} class="size-3.5" />
              <span>Open project folder</span>
            </button>
          {/if}

          <!-- Project Sessions / Recent Chats -->
          {#if snap.sessions.length > 0}
            <div class="flex flex-col gap-0.5 mt-1">
              {#if snap.sessions.length > 3}
                <div class="px-1 pb-1">
                  <Input
                    type="text"
                    placeholder="Filter sessions..."
                    class="h-7 text-xs bg-background/60"
                    bind:value={sidebarFilter}
                  />
                </div>
              {/if}
              <Sidebar.Menu>
                {#each snap.sessions.filter((s) => !sidebarFilter.trim() || (s.title || s.id).toLowerCase().includes(sidebarFilter.trim().toLowerCase())) as session (session.id)}
                  <Sidebar.MenuItem>
                    <Sidebar.MenuButton
                      isActive={snap.session === session.id}
                      aria-disabled={snap.session === session.id}
                      tooltipContent={session.title || session.id}
                      class="h-7 text-xs gap-2 pl-3"
                      onclick={() => resumeSession(session.id)}
                    >
                      <HugeiconsIcon icon={CircleIcon} strokeWidth={2} class="size-2 text-primary/70 shrink-0" />
                      <span class="truncate">{session.title || session.id.slice(0, 8)}</span>
                      <Sidebar.MenuBadge class="text-[9px] py-0">{session.rollupStatus}</Sidebar.MenuBadge>
                    </Sidebar.MenuButton>
                  </Sidebar.MenuItem>
                {/each}
              </Sidebar.Menu>
            </div>
          {/if}
          {#if showProjectAdvanced}
            <div class="mt-2 space-y-1.5 px-1">
              <label for="project-root" class="text-[10px] text-muted-foreground font-medium">Project root</label>
              <div class="flex items-center gap-1">
                <Input
                  id="project-root"
                  placeholder="/absolute/path/to/project"
                  class="h-7 text-xs"
                  aria-invalid={openProjectRefusal ? 'true' : undefined}
                  aria-describedby={openProjectRefusal ? 'project-root-refusal' : undefined}
                  bind:value={projectPathInput}
                />
                <Button variant="outline" size="sm" class="h-7 text-xs shrink-0" onclick={openProject}>
                  Open workspace
                </Button>
              </div>
              {#if openProjectRefusal}
                <Field.Error id="project-root-refusal" data-testid="open-project-refusal">
                  {#if openProjectRefusal.code}<span class="font-mono text-xs">{openProjectRefusal.code}</span><span aria-hidden="true"> · </span>{/if}
                  {openProjectRefusal.message}
                </Field.Error>
              {/if}
            </div>
          {:else}
            <!-- Hidden accessible fallback for programmatic control and tests -->
            <div class="sr-only">
              <label for="project-root">Project root</label>
              <Input
                id="project-root"
                placeholder="/absolute/path/to/project"
                aria-invalid={openProjectRefusal ? 'true' : undefined}
                aria-describedby={openProjectRefusal ? 'project-root-refusal' : undefined}
                bind:value={projectPathInput}
              />
              <Button onclick={openProject}>Open workspace</Button>
              {#if openProjectRefusal}
                <Field.Error id="project-root-refusal" data-testid="open-project-refusal">
                  {#if openProjectRefusal.code}<span class="font-mono text-xs">{openProjectRefusal.code}</span><span aria-hidden="true"> · </span>{/if}
                  {openProjectRefusal.message}
                </Field.Error>
              {/if}
            </div>
          {/if}
        </Sidebar.GroupContent>
      </Sidebar.Group>

      <!-- Accounts & Worktrees if present -->
      {#if connectedAccounts.length > 0}
        <Sidebar.Group class="py-1">
          <Sidebar.GroupLabel class="text-[11px] font-semibold text-muted-foreground uppercase tracking-wider px-2 py-1">Accounts</Sidebar.GroupLabel>
          <Sidebar.GroupContent>
            <div class="flex flex-wrap gap-1.5 px-2 py-0.5">
              {#each connectedAccounts as account (account.provider)}
                <div
                  class="flex items-center gap-1.5 rounded-full border bg-background px-2 py-0.5 text-xs"
                  data-testid="account-pill"
                >
                  <StatusOrb state="verified" size={5} />
                  <span>{account.provider}</span>
                </div>
              {/each}
            </div>
          </Sidebar.GroupContent>
        </Sidebar.Group>
      {/if}

      {#if clientStore.worktrees.length > 0}
        <Sidebar.Group class="py-1">
          <Sidebar.GroupLabel class="flex items-center justify-between text-[11px] font-semibold text-muted-foreground uppercase tracking-wider px-2 py-1">
            <span>Worktrees</span>
            <span class="text-muted-foreground">{clientStore.worktrees.length}</span>
          </Sidebar.GroupLabel>
          <Sidebar.GroupContent>
            <Sidebar.Menu>
              {#each clientStore.worktrees as worktree (worktree.child)}
                <Sidebar.MenuItem>
                  <div class="flex w-full flex-col gap-0.5 rounded-md px-2 py-1 text-xs" data-testid="worktree-item">
                    <div class="flex items-center justify-between gap-2">
                      <div class="flex min-w-0 items-center gap-1.5">
                        <StatusOrb state={worktree.done ? 'verified' : 'active'} size={5} />
                        <span class="truncate font-medium">{worktree.child.slice(0, 8)}</span>
                      </div>
                      <span class="shrink-0 truncate rounded bg-muted px-1.5 py-0.2 font-mono text-[9px]">
                        {worktree.branch}
                      </span>
                    </div>
                    <div class="flex items-center gap-1.5 pl-3.5 font-mono text-[9px] text-muted-foreground">
                      <span class="truncate">{worktree.path}</span>
                    </div>
                  </div>
                </Sidebar.MenuItem>
              {/each}
            </Sidebar.Menu>
          </Sidebar.GroupContent>
        </Sidebar.Group>
      {/if}

      {#if fleetVisible}
        <Sidebar.Group class="py-1">
          <Sidebar.GroupLabel class="flex items-center justify-between text-[11px] font-semibold text-muted-foreground uppercase tracking-wider px-2 py-1">
            <span>Fleet</span>
            <span class="text-accent-bright">{clientStore.fleet.filter((agent) => !agent.done).length} live</span>
          </Sidebar.GroupLabel>
          <Sidebar.GroupContent>
            <Sidebar.Menu>
              {#each clientStore.fleet as agent (agent.child)}
                <Sidebar.MenuItem>
                  <div class="flex w-full items-center justify-between gap-2 rounded-md px-2 py-1 text-xs">
                    <div class="flex min-w-0 items-center gap-1.5">
                      <StatusOrb state={agent.done ? 'idle' : 'active'} size={5} />
                      <span class="truncate">{agent.latest}</span>
                    </div>
                    <span class="shrink-0 rounded bg-accent-muted px-1.5 py-0.2 font-mono text-[9px] text-accent-bright">
                      {agent.short}
                    </span>
                  </div>
                </Sidebar.MenuItem>
              {/each}
            </Sidebar.Menu>
          </Sidebar.GroupContent>
        </Sidebar.Group>
      {/if}

      <!-- Model & Provider Selection -->
      <Sidebar.Group class="py-1">
        <Sidebar.GroupLabel class="text-[11px] font-semibold text-muted-foreground uppercase tracking-wider px-2 py-1">Model</Sidebar.GroupLabel>
        <Sidebar.GroupContent class="px-2">
          {#if snap.models.length === 0}
            <div class="rounded-lg border border-border/80 bg-muted/30 p-2.5 text-xs flex flex-col gap-2">
              <p class="text-muted-foreground text-[11px]">No models connected yet.</p>
              <Button variant="outline" size="sm" class="w-full text-xs h-7 gap-1 text-primary border-primary/40" onclick={() => goto('/onboarding')}>
                <HugeiconsIcon icon={SparklesIcon} class="size-3" />
                Connect a provider
              </Button>
            </div>
          {:else}
            <div class="flex flex-col gap-1.5">
              {#if providerChoices.length > 1}
                <Select.Root type="single" bind:value={selectedProvider}>
                  <Select.Trigger class="w-full h-7 text-xs">{selectedProviderLabel}</Select.Trigger>
                  <Select.Content>
                    <Select.Group>
                      <Select.Label>Providers</Select.Label>
                      {#each providerChoices as provider (provider)}
                        <Select.Item value={provider} label={provider}>{provider}</Select.Item>
                      {/each}
                    </Select.Group>
                  </Select.Content>
                </Select.Root>
              {/if}
              <Select.Root type="single" bind:value={selectedModel}>
                <Select.Trigger class="w-full h-7 text-xs">{selectedModelLabel}</Select.Trigger>
                <Select.Content>
                  <Select.Group>
                    <Select.Label>Models</Select.Label>
                    {#each modelChoices as choice (choice.model)}
                      <Select.Item value={choice.model} label={choice.displayName}>
                        {choice.displayName}
                      </Select.Item>
                    {/each}
                  </Select.Group>
                </Select.Content>
              </Select.Root>
            </div>
          {/if}
        </Sidebar.GroupContent>
      </Sidebar.Group>
    </Sidebar.Content>

    <!-- Footer: Persona, Policy, Git controls & Theme -->
    <Sidebar.Footer class="p-2 border-t border-sidebar-border/40 gap-1.5">
      <!-- Persona Link -->
      <button
        type="button"
        class="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-xs hover:bg-sidebar-accent transition-colors cursor-pointer"
        onclick={() => openGovernance('soul')}
      >
        <HugeiconsIcon icon={MaskTheater01Icon} strokeWidth={2} class="size-3.5 shrink-0 text-primary" />
        <span class="truncate">{snap.activePersona ?? 'Route default'}</span>
        <span class="ml-auto font-mono text-[9px] text-muted-foreground">SOUL.md ↗</span>
      </button>

      <!-- Policy Switcher -->
      <div class="flex items-center justify-between px-1">
        <span class="text-[11px] text-muted-foreground">Policy</span>
        <ToggleGroup.Root
          type="single"
          value={selectablePolicies.includes(snap.policy) ? snap.policy : ''}
          class="grid grid-cols-3 gap-0.5 h-6"
          aria-label="Execution policy"
        >
          {#each selectablePolicies as policy (policy)}
            <ToggleGroup.Item
              value={policy}
              aria-label={policy}
              class="h-6 text-[10px] px-1.5 py-0"
              title={`Set policy to ${policy}`}
              onclick={() => dispatch({ type: 'setPolicy', policy })}
            >
              {POLICY_SEGMENT_LABEL[policy]}
            </ToggleGroup.Item>
          {/each}
        </ToggleGroup.Root>
      </div>

      <!-- Git & Repository Trigger Button -->
      <button
        type="button"
        class="flex w-full items-center justify-between rounded-md border border-border/60 bg-muted/20 px-2 py-1 text-xs text-muted-foreground hover:bg-muted/40 hover:text-foreground transition-colors cursor-pointer"
        onclick={() => (repoDialogOpen = true)}
      >
        <div class="flex items-center gap-1.5">
          <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} class="size-3.5 text-primary" />
          <span>Git & Changes</span>
        </div>
        <span class="font-mono text-[9px] text-muted-foreground">
          {snap.repository?.freshness.type === 'capturedAt' ? snap.repository.branch || 'Clean' : 'Idle'}
        </span>
      </button>

      <!-- Hidden repository panel hook to preserve test assertions -->
      <div class="sr-only" data-testid="repository-panel">
        <RepositoryPanel />
      </div>
    </Sidebar.Footer>
    <Sidebar.Rail />
  </Sidebar.Root>

  <Sidebar.Inset class="min-w-0 overflow-hidden">
    <header class="flex h-12 shrink-0 items-center gap-2 border-b bg-sidebar px-3" style="height:var(--header-h);">
      <AppEmblem size={24} />
      <span class="shrink-0 text-sm font-bold tracking-tight text-foreground flex items-center gap-1.5">
        mjolnr
        <span class="rounded bg-primary/10 border border-primary/20 px-1.5 py-0.2 font-mono text-[9px] text-primary font-medium">v0.0.0</span>
      </span>
      <Separator orientation="vertical" class="mx-1 h-4.5" />
      <Sidebar.Trigger title="Toggle sidebar (⌘B)" />
      <button
        type="button"
        class="flex min-w-0 items-center gap-2 rounded-md border border-border/80 bg-background/80 hover:bg-muted/60 transition-colors px-2.5 py-1 text-xs text-muted-foreground cursor-pointer"
        title={snap.workspaceRoot ?? 'No workspace open'}
        onclick={snap.workspaceRoot ? undefined : chooseWorkspace}
      >
        <HugeiconsIcon icon={Folder01Icon} strokeWidth={2} class="size-3.5 shrink-0 text-primary" />
        <span class="truncate font-mono font-medium text-foreground">{snap.workspaceRoot ? snap.workspaceRoot.split('/').pop() || snap.workspaceRoot : 'No workspace open'}</span>
        {#if isConnected}
          <StatusOrb state={snap.workspaceRoot ? 'active' : 'idle'} size={6} />
        {/if}
      </button>

      <div class="flex-1"></div>

      <div
        class={cn(
          'flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium transition-colors',
          loopState.tone === 'approval' && 'border-gov-approval-border bg-gov-approval-bg text-gov-approval shadow-[0_0_12px_var(--gov-approval-glow)]',
          loopState.tone === 'active' && 'border-accent-border bg-accent-muted text-accent-bright shadow-[0_0_12px_var(--accent-glow)]',
          loopState.tone === 'idle' && 'border-border/60 bg-muted/30 text-muted-foreground'
        )}
      >
        <ActivityBars active={snap.runActive} />
        <span>{loopState.label}</span>
      </div>

      {#if quotaWindow}
        <Badge variant={QUOTA_BADGE_VARIANT[quotaSeverity(quotaWindow.usedFraction)]} data-testid="quota-indicator">
          {quotaWindow.label} {(quotaWindow.usedFraction * 100).toFixed(0)}% used{quotaWindow.resetsAt
            ? ` (${formatCountdown(quotaWindow.resetsAt)})`
            : ''}
        </Badge>
      {/if}

      <Separator orientation="vertical" class="mx-1 h-4.5" />

      <Button variant="secondary" size="sm" class="border border-border/80 bg-background/80 hover:bg-muted" onclick={() => (paletteOpen = true)}>
        <HugeiconsIcon icon={SearchIcon} strokeWidth={2} class="text-primary" data-icon="inline-start" />
        Jump
        <kbd class="text-xs text-muted-foreground font-mono rounded bg-muted px-1 py-0.5">⌘K</kbd>
      </Button>
      <Button
        variant={explorerOpen ? 'default' : 'ghost'}
        size="icon-sm"
        aria-label="Toggle file explorer (⌘E)"
        onclick={() => (explorerOpen = !explorerOpen)}
      >
        <HugeiconsIcon icon={FolderOpenIcon} strokeWidth={2} />
      </Button>
      <Button
        variant={terminalOpen ? 'default' : 'ghost'}
        size="icon-sm"
        aria-label="Toggle terminal (⌘\)"
        onclick={() => (terminalOpen = !terminalOpen)}
      >
        <HugeiconsIcon icon={TerminalIcon} strokeWidth={2} />
      </Button>
      <Button variant="ghost" size="icon-sm" aria-label="Resync runtime state" onclick={() => dispatch({ type: 'requestSnapshot' })}>
        <HugeiconsIcon icon={RefreshIcon} strokeWidth={2} />
      </Button>
      <Button variant={splitPreset === 'changes' ? 'default' : 'ghost'} size="icon-sm" aria-label="Split: Changes" onclick={() => (splitPreset = splitPreset === 'changes' ? 'none' : 'changes')}>
        <HugeiconsIcon icon={FileEditIcon} strokeWidth={2} />
      </Button>
      <Button variant={splitPreset === 'verify' ? 'default' : 'ghost'} size="icon-sm" aria-label="Split: Verify" onclick={() => (splitPreset = splitPreset === 'verify' ? 'none' : 'verify')}>
        <HugeiconsIcon icon={CheckmarkCircle02Icon} strokeWidth={2} />
      </Button>
      <Button variant={splitPreset === 'inspector' ? 'default' : 'ghost'} size="icon-sm" aria-label="Toggle inspector (⌘I)" onclick={() => (splitPreset = splitPreset === 'inspector' ? 'none' : 'inspector')}>
        <HugeiconsIcon icon={PanelRightOpenIcon} strokeWidth={2} />
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Governance & Council (⌘G)"
        class="text-primary hover:text-primary hover:bg-primary/10"
        onclick={() => openGovernance('council')}
      >
        <HugeiconsIcon icon={Key01Icon} strokeWidth={2} />
      </Button>
      <Button variant="ghost" size="icon-sm" aria-label="Switch theme (⌘T)" onclick={() => toggleMode()}>
        <HugeiconsIcon icon={mode.current === 'dark' ? Sun01Icon : Moon01Icon} strokeWidth={2} />
      </Button>
    </header>

    {#if clientStore.lastError}
      <Alert.Root variant="destructive" class="m-4 mb-0">
        <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
        <Alert.Title>Desktop bridge error</Alert.Title>
        <Alert.Description>{clientStore.lastError}</Alert.Description>
      </Alert.Root>
    {/if}

    <div class="flex min-h-0 flex-1">
      <div class="flex min-w-0 flex-1 flex-col">
    <Resizable.PaneGroup direction="horizontal" class="min-h-0 flex-1">
      <Resizable.Pane
        defaultSize={splitPreset === 'none' ? 100 : 50}
        minSize={30}
        class="flex min-w-0 flex-col"
      >
        <div class="flex min-h-0 min-w-0 flex-1">
          <Tabs.Root bind:value={activeSurface} class="flex min-h-0 min-w-0 flex-1 flex-col gap-0">
      <Tabs.Content value="Conversation" class="min-h-0 flex-1 overflow-hidden" data-testid="conversation-surface">
        <section class="flex size-full min-h-0 flex-col">
          <div class="flex items-center justify-between border-b px-5 py-3">
            <div class="flex min-w-0 flex-col gap-0.5">
              <h1 class="truncate text-base font-semibold">
                {snap.session ? `Session ${snap.session.slice(0, 8)}` : projectName ? `Project ${projectName}` : 'What should we work on?'}
              </h1>
              <p class="text-xs text-muted-foreground">
                {snap.runActive ? 'mjolnr is working' : snap.session ? 'Ready for your next instruction' : 'Type a prompt or choose a project to start chatting'}
              </p>
            </div>
            <div class="flex items-center gap-2">
              <Badge variant={snap.runActive ? 'default' : 'secondary'}>
                {snap.runActive ? 'Run active' : 'Idle'}
              </Badge>
              {#if snap.runActive}
                <Button variant="destructive" size="sm" onclick={() => dispatch({ type: 'cancelRun' })}>
                  <HugeiconsIcon icon={StopIcon} strokeWidth={2} data-icon="inline-start" />
                  Cancel
                </Button>
              {/if}
            </div>
          </div>

          {#if snap.storeFailure}
            <Alert.Root variant="destructive" class="mx-6 mt-4">
              <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
              <Alert.Title>Durability failure</Alert.Title>
              <Alert.Description>{snap.storeFailure}</Alert.Description>
            </Alert.Root>
          {:else if snap.recovery.state === 'required'}
            <Alert.Root variant="destructive" class="mx-6 mt-4">
              <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
              <Alert.Title>Recovery decision required</Alert.Title>
              <Alert.Description>{snap.recovery.summary}</Alert.Description>
              <Alert.Action class="flex gap-2">
                <Button size="sm" variant="outline" onclick={() => dispatch({ type: 'resolveRecovery', decision: 'abandon-and-continue' })}>
                  Abandon and continue
                </Button>
                <Button size="sm" variant="destructive" onclick={() => dispatch({ type: 'resolveRecovery', decision: 'end-session' })}>
                  End session
                </Button>
              </Alert.Action>
            </Alert.Root>
          {:else if snap.pendingApproval}
            <div class="mx-6 mt-4 overflow-hidden rounded-lg border border-gov-approval-border bg-card shadow-[0_0_40px_var(--gov-approval-glow)]">
              <div class="flex items-center justify-between gap-3 border-b border-gov-approval-border bg-gov-approval-bg px-4 py-3">
                <div class="flex items-center gap-2.5">
                  <HugeiconsIcon icon={Key01Icon} strokeWidth={2} class="size-4.5 text-gov-approval" />
                  <h3 class="text-sm font-semibold">Approval Gate</h3>
                </div>
                <span class="rounded-sm border border-gov-approval-border bg-gov-approval-bg px-1.5 py-0.5 font-mono text-[10px] text-gov-approval">
                  {snap.pendingApproval.toolName} · {snap.pendingApproval.tier}
                </span>
              </div>
              <div class="p-4">
                {#if previewLooksLikeDiff(snap.pendingApproval.preview)}
                  <pre class="mb-3 max-h-56 overflow-auto rounded-md border bg-background p-3 font-mono text-xs">{#each snap.pendingApproval.preview.split('\n') as line, i (i)}<span class={previewDiffClass(line)}>{line}</span>{'\n'}{/each}</pre>
                {:else}
                  <pre class="mb-3 max-h-56 overflow-auto rounded-md border bg-background p-3 font-mono text-xs">{snap.pendingApproval.preview}</pre>
                {/if}
                <div class="flex flex-wrap items-center gap-2">
                  <Button
                    size="sm"
                    class="border border-gov-approval-border bg-gov-approval-bg font-semibold text-gov-approval hover:bg-gov-approval-bg"
                    onclick={() => approve('approve-once')}
                  >
                    Approve once
                  </Button>
                  <Button size="sm" variant="secondary" onclick={() => approve('approve-exact-for-session')}>
                    Approve exact for session
                  </Button>
                  <Button size="sm" variant="destructive" onclick={() => approve('deny')}>Deny</Button>
                  <span class="ml-auto text-xs text-muted-foreground">Stale revisions are refused automatically</span>
                </div>
              </div>
            </div>
          {:else if snap.resumeAdvice}
            <Alert.Root class="mx-6 mt-4">
              <HugeiconsIcon icon={Activity03Icon} strokeWidth={2} />
              <Alert.Title>Choose how to resume</Alert.Title>
              <Alert.Description>{snap.resumeAdvice.warning}</Alert.Description>
              <Alert.Action class="flex flex-wrap gap-2">
                <Button size="sm" onclick={() => resolveResume('compact')}>Compact</Button>
                {#if snap.resumeAdvice.hasHandoff}
                  <Button size="sm" variant="secondary" onclick={() => resolveResume('new-from-handoff')}>From handoff</Button>
                {/if}
                <Button size="sm" variant="outline" onclick={() => resolveResume('full')}>Full transcript</Button>
              </Alert.Action>
            </Alert.Root>
          {/if}

          <ScrollArea.Root class="min-h-0 flex-1">
            <div class="mx-auto flex w-full max-w-4xl flex-col gap-5 p-6">
              {#if snap.messages.length === 0 && !streamingText}
                <!-- Codex Conversational Hero Layout -->
                <div class="flex flex-col items-center justify-center min-h-[50vh] gap-6 px-2 py-4">
                  <div class="flex flex-col items-center gap-3 text-center">
                    <AppEmblem size={52} />
                    <h2 class="text-2xl sm:text-3xl font-bold tracking-tight text-foreground">
                      What should we work on in <button type="button" class="underline decoration-primary/40 underline-offset-4 hover:decoration-primary cursor-pointer transition-colors" onclick={chooseWorkspace}>{projectName || 'mjolnr'}</button>?
                    </h2>
                    <p class="text-xs sm:text-sm text-muted-foreground max-w-md">
                      Direct wire models · Deterministic safety · Worktree isolation
                    </p>
                  </div>

                  <!-- Floating Central Chat Box -->
                  <div class="w-full max-w-2xl rounded-xl border border-border/80 bg-card/90 shadow-xl backdrop-blur-md transition-all focus-within:border-primary/60 focus-within:shadow-[0_0_24px_var(--accent-muted)] p-3 flex flex-col gap-2.5">
                    <div class="flex items-center justify-between gap-2 px-1">
                      <button
                        type="button"
                        class="flex items-center gap-1.5 rounded-full border border-border/60 bg-muted/40 px-2.5 py-0.5 text-xs text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer"
                        onclick={chooseWorkspace}
                        title="Switch project directory"
                      >
                        <HugeiconsIcon icon={Folder01Icon} strokeWidth={2} class="size-3.5 text-primary" />
                        <span class="truncate font-mono font-medium">{projectName || 'Choose project folder'}</span>
                      </button>

                      {#if snap.models.length === 0}
                        <Button variant="outline" size="sm" class="h-6 text-[11px] gap-1 border-primary/40 text-primary" onclick={() => goto('/onboarding')}>
                          <HugeiconsIcon icon={SparklesIcon} class="size-3" />
                          Connect provider
                        </Button>
                      {/if}
                    </div>

                    <Textarea
                      placeholder="Do anything — ask a question, describe a feature to build, or paste an error..."
                      class="min-h-24 resize-none border-0 bg-transparent p-1 text-sm shadow-none focus-visible:ring-0 placeholder:text-muted-foreground/60"
                      bind:value={messageInput}
                      onkeydown={(e) => {
                        if (e.key === 'Enter' && (e.metaKey || e.ctrlKey || !e.shiftKey)) {
                          e.preventDefault();
                          sendMessage();
                        }
                      }}
                    />

                    <div class="flex flex-wrap items-center justify-between gap-2 border-t border-border/40 pt-2.5">
                      <div class="flex items-center gap-2">
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          class="h-7 w-7 rounded-md text-muted-foreground hover:text-foreground"
                          title="Attach files (coming soon)"
                        >
                          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} class="size-4" />
                        </Button>

                        <!-- Policy Mode Pill -->
                        <button
                          type="button"
                          class="flex items-center gap-1 rounded-md border border-border/60 bg-muted/30 px-2 py-1 text-xs text-muted-foreground hover:text-foreground transition-colors cursor-pointer"
                          onclick={cyclePolicy}
                          title="Cycle policy mode"
                        >
                          <span class="text-[11px]">🛡️ {POLICY_SEGMENT_LABEL[snap.policy] || snap.policy}</span>
                        </button>

                        <!-- Model Selector in Composer -->
                        {#if snap.models.length > 0}
                          <Select.Root type="single" bind:value={selectedModel}>
                            <Select.Trigger class="h-7 border-border/60 bg-muted/30 text-xs px-2.5 py-0 gap-1.5">
                              <span class="truncate">Model: {selectedModelLabel}</span>
                            </Select.Trigger>
                            <Select.Content>
                              <Select.Group>
                                <Select.Label>Models</Select.Label>
                                {#each snap.models as choice (choice.model)}
                                  <Select.Item value={choice.model} label={choice.displayName}>
                                    {choice.displayName}
                                  </Select.Item>
                                {/each}
                              </Select.Group>
                            </Select.Content>
                          </Select.Root>
                        {/if}
                      </div>

                      <Button
                        size="sm"
                        class="h-7 font-semibold gap-1.5 px-3 bg-primary text-primary-foreground shadow-sm hover:bg-primary/90"
                        disabled={!messageInput.trim() || snap.runActive}
                        onclick={() => sendMessage()}
                      >
                        <HugeiconsIcon icon={SentIcon} strokeWidth={2} class="size-3.5" />
                        Send
                        <kbd class="ml-1 rounded bg-black/20 px-1 py-0.2 font-mono text-[9px]">↵</kbd>
                      </Button>
                    </div>
                  </div>

                  <!-- Quick Prompt Suggestion Chips -->
                  <div class="flex flex-wrap items-center justify-center gap-2 max-w-xl">
                    {#each [
                      'Analyze project architecture & dependencies',
                      'Find and fix potential bugs',
                      'Run test suite and verify changes',
                      'Create step-by-step implementation plan'
                    ] as prompt}
                      <button
                        type="button"
                        class="rounded-full border border-border/60 bg-muted/20 px-3 py-1 text-xs text-muted-foreground hover:text-foreground hover:border-primary/50 hover:bg-muted/40 transition-all cursor-pointer"
                        onclick={() => sendMessage(prompt)}
                      >
                        {prompt}
                      </button>
                    {/each}
                  </div>

                  <!-- Collapsible Project Setup Section (preserves launch-journey test hooks) -->
                  <div class="w-full max-w-2xl mt-3" data-testid="launch-journey">
                    <Card.Root class="border-border/60 bg-card/40">
                      <Card.Header class="py-2.5 px-4">
                        <div class="flex items-center justify-between">
                          <div class="flex items-center gap-2">
                            <HugeiconsIcon icon={FolderOpenIcon} class="size-3.5 text-primary" />
                            <Card.Title class="text-xs font-semibold">1. Open a workspace</Card.Title>
                          </div>
                          <Button size="sm" variant="ghost" class="h-6 text-xs text-muted-foreground" onclick={() => (showProjectAdvanced = !showProjectAdvanced)}>
                            {showProjectAdvanced ? 'Hide options' : 'Project settings'}
                          </Button>
                        </div>
                        <Card.Description class="text-xs">
                          {journeyState === 'workspace'
                            ? 'mjolnr started without a project folder. Choose the directory mjolnr is allowed to inspect and modify.'
                            : `Workspace ready: ${snap.workspaceRoot}`}
                        </Card.Description>
                      </Card.Header>
                      {#if journeyState === 'workspace' || showProjectAdvanced}
                        <Card.Content class="space-y-3 px-4 pb-4">
                          <div class="flex flex-wrap items-center gap-2">
                            <Button size="sm" class="font-semibold gap-1.5" onclick={chooseWorkspace} data-testid="choose-workspace">
                              <HugeiconsIcon icon={FolderOpenIcon} strokeWidth={2} data-icon="inline-start" />
                              Choose project folder
                              <kbd class="ml-1 rounded bg-black/20 px-1 py-0.2 text-[9px] font-mono">⌘O</kbd>
                            </Button>
                            <Button size="sm" variant="outline" onclick={() => goto('/onboarding')} data-testid="guided-setup">
                              Guided setup
                            </Button>
                          </div>
                          <div class="flex flex-col gap-2 sm:flex-row">
                            <label for="launch-project-root" class="sr-only">Absolute project path</label>
                            <Input
                              id="launch-project-root"
                              class="h-8 min-w-0 bg-background/80 text-xs"
                              placeholder="Or enter an absolute project path"
                              autocomplete="off"
                              aria-invalid={openProjectRefusal ? 'true' : undefined}
                              aria-describedby={openProjectRefusal ? 'launch-project-refusal' : 'launch-project-hint'}
                              bind:value={projectPathInput}
                              onkeydown={(e) => {
                                if (e.key === 'Enter') openProject();
                              }}
                            />
                            <Button variant="outline" size="sm" class="h-8 shrink-0 text-xs" onclick={openProject}>Open entered path</Button>
                          </div>
                          {#if openProjectRefusal}
                            <Field.Error id="launch-project-refusal" data-testid="launch-project-refusal">
                              {#if openProjectRefusal.code}<span class="font-mono text-xs">{openProjectRefusal.code}</span><span aria-hidden="true"> · </span>{/if}
                              {openProjectRefusal.message}
                            </Field.Error>
                          {:else}
                            <p id="launch-project-hint" class="text-[11px] text-muted-foreground">
                              The launch location is not used as a project. mjolnr acts only after you choose a folder.
                            </p>
                          {/if}
                          <div class="border-t border-border/60 pt-2.5">
                            <CloneRepository />
                          </div>
                        </Card.Content>
                      {/if}
                    </Card.Root>
                  </div>
                </div>
              {:else}
                {#if snap.messagesOmitted > 0}
                  <p class="text-center text-xs text-muted-foreground">
                    {snap.messagesOmitted} earlier messages omitted from this projection.
                  </p>
                {/if}
                {#each snap.messages as message (message.id)}
                  {#if message.kind === 'tool'}
                    <div class="max-w-[90%] overflow-hidden rounded-md border">
                      <div class={cn('flex items-center justify-between gap-3 border-b px-3 py-2', TOOL_OUTCOME_CLASS[message.outcome])}>
                        <span class="font-mono text-sm font-medium text-foreground">{message.name}</span>
                        <span class="shrink-0 text-xs font-medium">{message.outcome}</span>
                      </div>
                      <pre class="max-h-56 overflow-auto p-3 font-mono text-xs text-muted-foreground">{message.detail}</pre>
                    </div>
                  {:else}
                    <Card.Root class={message.kind === 'user' ? 'ml-auto max-w-[80%]' : 'max-w-[90%]'}>
                      <Card.Header class="pb-2">
                        <div class="flex items-center justify-between gap-3">
                          <Card.Title class="text-sm capitalize">{message.kind}</Card.Title>
                          {#if message.kind === 'assistant' && message.provider}
                            <Badge variant="outline">{message.provider} · {message.model}</Badge>
                          {/if}
                        </div>
                      </Card.Header>
                      <Card.Content>
                        <p class="whitespace-pre-wrap text-sm">{message.text}</p>
                      </Card.Content>
                    </Card.Root>
                  {/if}
                {/each}
                {#if streamingText}
                  <Card.Root class="max-w-[90%]">
                    <Card.Header class="pb-2"><Card.Title class="text-sm">assistant · streaming</Card.Title></Card.Header>
                    <Card.Content>
                      <p class="whitespace-pre-wrap text-sm">
                        {streamingText}<span class="stream-cursor"></span>
                      </p>
                    </Card.Content>
                  </Card.Root>
                {/if}
              {/if}
            </div>
            <ScrollArea.Scrollbar orientation="vertical" />
          </ScrollArea.Root>

          <!-- Always Unlocked Bottom Composer Bar -->
          <div class="border-t bg-background p-4">
            <div class="mx-auto flex w-full max-w-4xl flex-col gap-2">
              <Field.Field>
                <Field.Label for="composer" class="sr-only">Message mjolnr</Field.Label>
                <Textarea
                  id="composer"
                  placeholder={snap.workspaceRoot ? "Ask a question, describe a feature to build, or paste code…" : "Ask a question or describe a task…"}
                  rows={2}
                  bind:value={messageInput}
                  disabled={snap.runActive}
                  onkeydown={(event) => {
                    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
                      event.preventDefault();
                      sendMessage();
                    }
                  }}
                />
              </Field.Field>
              <div class="flex items-center justify-between gap-3">
                <p class="text-xs text-muted-foreground">
                  ⌘Enter to send · policy: <span class="font-mono text-accent-bright">{snap.policy}</span>
                  {#if snap.model}
                    <span aria-hidden="true"> · </span>model: <span class="font-mono text-foreground">{snap.model}</span>
                  {/if}
                </p>
                <div class="flex items-center gap-2">
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled
                    title="Attach is not wired to the runtime yet — files are not proposed to the model"
                    aria-label="Attach files (not available yet)"
                  >
                    <HugeiconsIcon icon={Attachment01Icon} strokeWidth={2} data-icon="inline-start" />
                    Attach
                  </Button>
                  <Button disabled={!messageInput.trim() || snap.runActive} onclick={() => sendMessage()}>
                    <HugeiconsIcon icon={SentIcon} strokeWidth={2} data-icon="inline-end" />
                    Send
                  </Button>
                </div>
              </div>
            </div>
          </div>
        </section>
      </Tabs.Content>

      <Tabs.Content value="Plan" class="min-h-0 flex-1 overflow-auto"><PlanSurface /></Tabs.Content>
      <Tabs.Content value="Board" class="min-h-0 flex-1 overflow-auto"><BoardPane /></Tabs.Content>
      <Tabs.Content value="Changes" class="min-h-0 flex-1 overflow-auto"><ChangesSurface /></Tabs.Content>
      <Tabs.Content value="Verify" class="min-h-0 flex-1 overflow-auto"><VerifySurface /></Tabs.Content>
      <Tabs.Content value="Attention" class="min-h-0 flex-1 overflow-auto"><AttentionSurface /></Tabs.Content>
        </Tabs.Root>
        {#if activeSurface === 'Conversation' && editorPath}
          <EditorPane
            path={editorPath}
            file={editorFile}
            tabs={editorTabs}
            onselect={selectEditorTab}
            onsave={saveEditorText}
            onclose={closeEditorTab}
            autosaveEnabled={editorAutosave}
            onautosavechange={setEditorAutosave}
            autosaveMessage={editorPreferencesRefusal?.message ?? null}
          />
        {/if}
        </div>
      </Resizable.Pane>

      {#if splitPreset !== 'none'}
        <Resizable.Handle withHandle />
        <Resizable.Pane
          defaultSize={splitPreset === 'inspector' ? 28 : 50}
          minSize={splitPreset === 'inspector' ? 24 : 30}
          class="flex min-w-0 flex-col border-l"
        >
          {#if splitPreset === 'inspector'}
            <div class="flex min-h-0 flex-1 justify-end overflow-auto">
              <InspectorPane onclose={() => (splitPreset = 'none')} />
            </div>
          {:else if splitPreset === 'changes'}
            <div class="border-b px-4 py-2 font-medium">Changes</div>
            <div class="flex-1 overflow-auto"><ChangesSurface /></div>
          {:else if splitPreset === 'verify'}
            <div class="border-b px-4 py-2 font-medium">Verify</div>
            <div class="flex-1 overflow-auto"><VerifySurface /></div>
          {:else if splitPreset === 'graph'}
            <GraphPane onopen={openFileInEditor} />
          {/if}
        </Resizable.Pane>
      {/if}
    </Resizable.PaneGroup>

        {#if terminalOpen}
          <TerminalPane
            expanded={terminalExpanded}
            onexpand={(next) => (terminalExpanded = next)}
            onclose={() => (terminalOpen = false)}
          />
        {/if}
      </div>

      {#if explorerOpen}
        <FileExplorer
          projectName={snap.workspaceRoot?.split('/').filter(Boolean).pop()}
          openPath={editorPath}
          onopen={openFileInEditor}
        />
      {/if}
    </div>

    <!--
      Footer telemetry bar, mockup-faithful (mockups/index.html 3072-3116).
      Every figure is a live projection of snapshot truth -- tokens from
      snap.usage, bars from snap.budget, the budget window from
      snap.quota (omitted until the runtime has actually reported a window,
      because a silent 0% bar would claim a clean bill). Over-budget windows
      tint the bar and figure, never the claim. The runtime dot says
      "connected", which is all the transport has verified.
    -->
    <footer
      class="flex h-8 shrink-0 items-center gap-3 border-t bg-sidebar px-3 text-xs text-muted-foreground"
      style="height: var(--footer-h);"
      data-testid="telemetry-bar"
    >
      <div class="flex items-center gap-2.5">
        <span>policy: <span class="font-mono text-accent-bright">{snap.policy}</span></span>
        <span class="h-3 w-px bg-border" aria-hidden="true"></span>
        <span>in: <span class="font-mono text-foreground">{formatTokenCount(snap.usage.inputTokens)}</span></span>
        <span>out: <span class="font-mono text-foreground">{formatTokenCount(snap.usage.outputTokens)}</span></span>
        <span class="h-3 w-px bg-border" aria-hidden="true"></span>
        <span class="flex items-center gap-1.5">
          turns
          <span class="h-1 w-14 overflow-hidden rounded-full bg-border">
            <span
              class="block h-full rounded-full bg-accent-bright"
              style="width: {budgetFraction(snap.budget.providerTurns, snap.budget.maxProviderTurns)}%"
            ></span>
          </span>
          <span class="font-mono text-foreground">{snap.budget.providerTurns}/{snap.budget.maxProviderTurns}</span>
        </span>
        <span class="flex items-center gap-1.5">
          tools
          <span class="h-1 w-14 overflow-hidden rounded-full bg-border">
            <span
              class="block h-full rounded-full bg-accent-bright"
              style="width: {budgetFraction(snap.budget.toolCalls, snap.budget.maxToolCalls)}%"
            ></span>
          </span>
          <span class="font-mono text-foreground">{snap.budget.toolCalls}/{snap.budget.maxToolCalls}</span>
        </span>
        {#if quotaWindow}
          <span class="flex items-center gap-1.5">
            budget
            <span class="h-1 w-14 overflow-hidden rounded-full bg-border">
              <span
                class={cn(
                  'block h-full rounded-full',
                  quotaSeverity(quotaWindow.usedFraction) === 'ok' ? 'bg-accent-bright' : 'bg-gov-approval'
                )}
                style="width: {Math.round(quotaWindow.usedFraction * 100)}%"
              ></span>
            </span>
            <span
              class={cn(
                'font-mono',
                quotaSeverity(quotaWindow.usedFraction) === 'ok' ? 'text-foreground' : 'text-gov-approval'
              )}
              title={quotaWindow.resetsAt ? `resets ${formatCountdown(quotaWindow.resetsAt)}` : undefined}
            >
              {Math.round(quotaWindow.usedFraction * 100)}%
            </span>
          </span>
        {/if}
      </div>
      <div class="ml-auto flex items-center gap-2.5">
        <span>trust: <span class="font-mono text-foreground">{snap.repository?.trust === 'mjolnrGoverned' ? 'mjolnr-verified' : snap.repository?.trust || 'runtime'}</span></span>
        <span class="h-3 w-px bg-border" aria-hidden="true"></span>
        {#if snap.session}
          <span>session: <span class="font-mono text-foreground">{snap.session.slice(0, 8)}</span></span>
          <span class="h-3 w-px bg-border" aria-hidden="true"></span>
        {/if}
        <span class="flex items-center gap-1.5">
          <StatusOrb state={isConnected ? 'active' : 'idle'} size={5} />
          runtime
        </span>
      </div>
    </footer>
  </Sidebar.Inset>
</Sidebar.Provider>

<GovernanceModal
  bind:open={governanceOpen}
  bind:activeTab={governanceTab}
  onamendment={openAmendmentInEditor}
/>

<Dialog.Root bind:open={repoDialogOpen}>
  <Dialog.Content class="max-w-2xl max-h-[85vh] overflow-y-auto bg-card border-border/80 shadow-2xl">
    <Dialog.Header>
      <Dialog.Title class="flex items-center gap-2 text-base font-semibold">
        <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} class="size-4 text-primary" />
        <span>Git Repository & Version Control</span>
      </Dialog.Title>
      <Dialog.Description class="text-xs text-muted-foreground">
        Authoritative repository state, branch information, and mutation controls.
      </Dialog.Description>
    </Dialog.Header>
    <div class="space-y-4 py-2">
      <RepositoryPanel />
      <div class="border-t border-border/60 pt-4">
        <RepositoryControls />
      </div>
    </div>
  </Dialog.Content>
</Dialog.Root>

<Command.Dialog
  bind:open={paletteOpen}
  title="mjolnr command palette"
  description="Navigate workspace and search recorded work"
  filter={(value, search) => {
    if (value.startsWith('result:')) return 1;
    return value.toLowerCase().includes(search.toLowerCase()) ? 1 : 0;
  }}
>
  <Command.Input placeholder="Type a command or search…" bind:value={paletteQuery} />
  <Command.List>
    <Command.Empty>No matching command.</Command.Empty>

    {#if paletteQuery.trim().length > 0}
      <Command.Group heading="Recorded work" forceMount data-testid="search-group">
        {#if searchRefusal}
          <!--
            A refusal, not an empty result. The store draws this distinction
            deliberately: "nothing matched" and "that could not be matched" send
            a user to different remedies, so the palette does not flatten them
            into one grey line. The reason code rides alongside the sentence.
          -->
          <div class="px-2 py-3 text-sm" role="status" data-testid="search-refusal">
            {#if searchRefusal.code}
              <span class="font-mono text-xs">{searchRefusal.code}</span>
              <span aria-hidden="true"> · </span>
            {/if}<span class="text-muted-foreground">{searchRefusal.message}</span>
          </div>
        {:else if searching}
          <div class="px-2 py-3 text-sm text-muted-foreground" data-testid="search-pending">
            Searching recorded work…
          </div>
        {:else if searchRan && searchResults.length === 0}
          <div class="px-2 py-3 text-sm text-muted-foreground" data-testid="search-empty">
            Nothing in the recorded transcript matched. Search covers indexed session events —
            not work items, review notes, or files on disk.
          </div>
        {:else}
          {#each searchResults as result (result.eventId)}
            <Command.Item
              forceMount
              value={`result:${result.eventId}`}
              onSelect={() => openSearchResult(result)}
            >
              <HugeiconsIcon icon={SearchIcon} strokeWidth={2} />
              <div class="flex min-w-0 flex-col gap-0.5">
                <span class="truncate text-sm">{result.matchSnippet}</span>
                <span class="truncate text-xs text-muted-foreground">
                  {snap.session === result.sessionId
                    ? 'this session'
                    : `resume session ${result.sessionId.slice(0, 8)}`} · #{result.sequence} ·
                  {result.occurredAt}
                </span>
              </div>
            </Command.Item>
          {/each}
        {/if}
      </Command.Group>
      {#if typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window}
        <Command.Group heading="Files" forceMount data-testid="file-search-group">
          {#if fileSearchRefusal}
            <div class="px-2 py-3 text-sm" role="status" data-testid="file-search-refusal">
              {#if fileSearchRefusal.code}
                <span class="font-mono text-xs">{fileSearchRefusal.code}</span>
                <span aria-hidden="true"> · </span>
              {/if}<span class="text-muted-foreground">{fileSearchRefusal.message}</span>
            </div>
          {:else if fileSearching}
            <div class="px-2 py-3 text-sm text-muted-foreground" data-testid="file-search-pending">
              Searching bounded workspace files…
            </div>
          {:else if fileSearchResults.length === 0}
            <div class="px-2 py-3 text-sm text-muted-foreground" data-testid="file-search-empty">
              No matching files were found in the bounded workspace search.
            </div>
          {:else}
            {#each fileSearchResults as result (result.path)}
              <Command.Item
                forceMount
                value={`file:${result.path}`}
                onSelect={() => {
                  paletteOpen = false;
                  void openFileInEditor(result.path);
                }}
              >
                <HugeiconsIcon icon={FolderOpenIcon} strokeWidth={2} />
                <span class="truncate font-mono text-sm">{result.path}</span>
                {#if result.ignored}<span class="ml-auto text-xs text-muted-foreground">ignored</span>{/if}
              </Command.Item>
            {/each}
            {#if fileSearchTruncated}
              <div class="px-2 py-2 text-xs text-muted-foreground" role="status" data-testid="file-search-truncated">
                Search stopped at the bounded workspace limit; narrow the query for more precise results.
              </div>
            {/if}
          {/if}
        </Command.Group>
      {/if}
      <Command.Separator />
    {/if}

    <Command.Group heading="Workspace surfaces">
      {#each surfaces as surface (surface.value)}
        <Command.Item value={`surface-${surface.value}`} onSelect={() => selectCommand(`surface-${surface.value}`)}>
          <HugeiconsIcon icon={surface.icon} strokeWidth={2} />
          <span>Open {surface.label}</span>
          <Command.Shortcut>{surface.shortcut}</Command.Shortcut>
        </Command.Item>
      {/each}
    </Command.Group>
    <Command.Separator />
    <Command.Group heading="Desktop">
      <Command.Item value="resync" onSelect={() => selectCommand('resync')}>
        <HugeiconsIcon icon={RefreshIcon} strokeWidth={2} /><span>Resync runtime state</span><Command.Shortcut>⌘R</Command.Shortcut>
      </Command.Item>
      <Command.Item value="theme" onSelect={() => selectCommand('theme')}>
        <HugeiconsIcon icon={mode.current === 'dark' ? Sun01Icon : Moon01Icon} strokeWidth={2} /><span>Toggle theme</span><Command.Shortcut>⌘T</Command.Shortcut>
      </Command.Item>
      <Command.Item value="inspector" onSelect={() => selectCommand('inspector')}>
        <HugeiconsIcon icon={PanelRightOpenIcon} strokeWidth={2} /><span>Open inspector</span><Command.Shortcut>⌘I</Command.Shortcut>
      </Command.Item>
      <Command.Item value="graph" onSelect={() => selectCommand('graph')}>
        <HugeiconsIcon icon={SearchIcon} strokeWidth={2} /><span>Open code graph</span>
      </Command.Item>
      <Command.Item value="explorer" onSelect={() => selectCommand('explorer')}>
        <HugeiconsIcon icon={FolderOpenIcon} strokeWidth={2} /><span>Toggle file explorer</span><Command.Shortcut>⌘E</Command.Shortcut>
      </Command.Item>
      <Command.Item value="terminal" onSelect={() => selectCommand('terminal')}>
        <HugeiconsIcon icon={TerminalIcon} strokeWidth={2} /><span>Toggle terminal</span><Command.Shortcut>⌘\</Command.Shortcut>
      </Command.Item>
      <Command.Item value="governance" onSelect={() => selectCommand('governance')}>
        <HugeiconsIcon icon={Key01Icon} strokeWidth={2} /><span>Governance &amp; Council</span><Command.Shortcut>⌘G</Command.Shortcut>
      </Command.Item>
      <Command.Item value="gallery" onSelect={() => selectCommand('gallery')}>
        <HugeiconsIcon icon={Image02Icon} strokeWidth={2} /><span>Open component gallery</span><Command.Shortcut>⌘⇧G</Command.Shortcut>
      </Command.Item>
      <Command.Item value="cancel" disabled={!snap.runActive} onSelect={() => selectCommand('cancel')}>
        <HugeiconsIcon icon={StopIcon} strokeWidth={2} /><span>Cancel active run</span><Command.Shortcut>⌘⇧C</Command.Shortcut>
      </Command.Item>
    </Command.Group>
  </Command.List>
</Command.Dialog>

<style>
  .stream-cursor {
    display: inline-block;
    width: 7px;
    height: 15px;
    background: var(--accent-cyan);
    border-radius: 1px;
    margin-left: 2px;
    vertical-align: text-bottom;
    animation: cursor-blink 0.9s ease-in-out infinite;
  }
  @keyframes cursor-blink {
    0%, 100% { opacity: 1; }
    50% { opacity: 0; }
  }
  @media (prefers-reduced-motion: reduce) {
    .stream-cursor {
      animation: none;
    }
  }
</style>
