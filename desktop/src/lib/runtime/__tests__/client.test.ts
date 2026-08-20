import { describe, it, expect, beforeEach } from 'vitest';
import { MjolnrClient, describeRefusal, isSearchRefusal } from '../client.svelte';
import type { ClientSnapshot } from '../contract';
import { NO_PROJECT_REPOSITORY, NO_REVIEW_THREADS } from '../contract';

const sampleSnapshot: ClientSnapshot = {
  revision: 1,
  session: '0190d5f0-test-session',
  provider: 'anthropic',
  model: 'claude-3-5-sonnet',
  workspaceRoot: '/test/root',
  policy: 'ask',
  runActive: false,
  usage: { inputTokens: 100, outputTokens: 50 },
  budget: { providerTurns: 1, maxProviderTurns: 25, toolCalls: 0, maxToolCalls: 100 },
  messages: [
    { kind: 'user', id: 'u1', text: 'Hello mjolnr', textTruncated: false },
    { kind: 'assistant', id: 'a1', text: 'Hello human', textTruncated: false, toolCalls: [] }
  ],
  messagesOmitted: 0,
  recovery: { state: 'clean' },
  models: [
    { provider: 'anthropic', model: 'claude-3-5-sonnet', displayName: 'Claude 3.5 Sonnet' }
  ],
  personas: [],
  souls: [],
  routes: [],
  accounts: [],
  sessions: [
    {
      id: '0190d5f0-test-session',
      title: 'Test Session',
      status: 'active',
      rollupStatus: 'running',
      provider: 'anthropic',
      model: 'claude-3-5-sonnet',
      updatedAt: '2026-07-28',
      eventCount: 2,
      leased: true
    }
  ],
  repository: NO_PROJECT_REPOSITORY,
  reviewThreads: NO_REVIEW_THREADS
};

describe('MjolnrClient behavior boundaries', () => {
  let client: MjolnrClient;

  beforeEach(() => {
    client = new MjolnrClient();
  });

  it('reports disconnected outside Tauri browser environment without manufacturing fake state', () => {
    expect(client.connected).toBe(false);
    expect(client.lastError).toContain('Tauri IPC unavailable');
    expect(client.snapshot.session).toBeUndefined();
  });

  it('replaces snapshot state on snapshot update', () => {
    client.handleUpdate({ type: 'snapshot', snapshot: sampleSnapshot });
    expect(client.snapshot.session).toBe('0190d5f0-test-session');
    expect(client.snapshot.revision).toBe(1);
    expect(client.snapshot.messages.length).toBe(2);
  });

  it('accumulates activity events without mutating authoritative snapshot messages', () => {
    client.handleUpdate({ type: 'snapshot', snapshot: sampleSnapshot });
    const initialMsgCount = client.snapshot.messages.length;

    client.handleUpdate({
      type: 'event',
      sequence: 1,
      event: { activity: 'textDelta', run: 'run-1', text: ' streaming delta', textTruncated: false }
    });

    expect(client.snapshot.messages.length).toBe(initialMsgCount);
    expect(client.streamingText).toBe(' streaming delta');
    expect(client.activityFeed.length).toBe(1);
  });

  it('replaces snapshot state on resync update', () => {
    const resyncSnapshot = { ...sampleSnapshot, revision: 5 };
    client.handleUpdate({ type: 'resync', missed: 3, snapshot: resyncSnapshot });
    expect(client.snapshot.revision).toBe(5);
    expect(client.resyncCount).toBe(3);
  });

  it('separates pending approval from recovery state cleanly', () => {
    const approvalSnapshot: ClientSnapshot = {
      ...sampleSnapshot,
      pendingApproval: {
        id: 'app-1',
        toolName: 'run_command',
        tier: 'execute',
        preview: 'rm -rf /'
      },
      recovery: { state: 'clean' }
    };
    client.handleUpdate({ type: 'snapshot', snapshot: approvalSnapshot });
    expect(client.snapshot.pendingApproval).toBeDefined();
    expect(client.snapshot.recovery.state).toBe('clean');
  });

  it('clears streaming text on runFinished', () => {
    client.handleUpdate({
      type: 'event',
      sequence: 1,
      event: { activity: 'textDelta', run: 'run-1', text: 'live delta', textTruncated: false }
    });
    expect(client.streamingText).toBe('live delta');

    client.handleUpdate({
      type: 'event',
      sequence: 2,
      event: { activity: 'runFinished', run: 'run-1', reason: 'stop' }
    });
    expect(client.streamingText).toBe('');
  });
});

/**
 * Tauri rejects an `invoke` with the *serialized* backend error, not an
 * `Error`. Reading `err.message` off that object yields `undefined` and
 * `String(err)` yields `"[object Object]"`, which is what every refusal used to
 * look like by the time it reached a surface.
 */
describe('refusal normalization', () => {
  it('reads the reason code and message out of a typed refusal', () => {
    const refused = {
      type: 'refused',
      detail: {
        code: 'WORKSPACE_ROOT_LOCKED',
        message: 'a session is already open on this workspace root'
      }
    };
    expect(describeRefusal(refused)).toEqual({
      code: 'WORKSPACE_ROOT_LOCKED',
      message: 'a session is already open on this workspace root'
    });
  });

  it('keeps the message when the runtime supplied no code, without inventing one', () => {
    const refused = { type: 'refused', detail: { code: null, message: 'the runtime is closed' } };
    expect(describeRefusal(refused)).toEqual({ code: null, message: 'the runtime is closed' });
  });

  it('handles the newtype variants that carry a bare string', () => {
    expect(
      describeRefusal({ type: 'initialization', detail: 'initialization failed: open SQLite store' })
    ).toEqual({
      code: null,
      message: 'initialization failed: open SQLite store'
    });
  });

  it('handles a real thrown Error, which is not a backend refusal', () => {
    expect(describeRefusal(new Error('module load failed'))).toEqual({
      code: null,
      message: 'module load failed'
    });
  });

  it('never yields "[object Object]" for an unrecognised object', () => {
    const result = describeRefusal({ type: 'disconnected' });
    expect(result.message).not.toContain('[object Object]');
    expect(result.message).toBe('disconnected');
  });

  it('falls back to a string form rather than an empty message', () => {
    expect(describeRefusal(null).message).toBe('null');
    expect(describeRefusal('plain string').message).toBe('plain string');
  });
  /**
   * A search refusal and an empty page are different answers and must stay
   * different at the type level, because the whole reason `StoreError::Refused`
   * exists is that "nothing matched" and "that could not be matched" send a
   * user to different remedies. A guard that treated an empty page as a
   * refusal would collapse them back.
   */
  it('tells an empty search page apart from a search refusal', () => {
    expect(isSearchRefusal({ items: [] })).toBe(false);
    expect(isSearchRefusal({ items: [], nextCursor: 'more' })).toBe(false);
    expect(isSearchRefusal({ code: 'WORKSPACE_SEARCH_REFUSED', message: 'too short' })).toBe(true);
    expect(isSearchRefusal({ code: null, message: 'browser mode' })).toBe(true);
  });

  /**
   * Search is a question, not a command. Outside Tauri it answers with a
   * refusal — and does NOT set `lastError`, because routing every short query
   * to the global Attention alert would make typing an error state.
   */
  it('refuses a search outside Tauri without raising the global error', async () => {
    const fresh = new MjolnrClient();
    fresh.lastError = null;

    const result = await fresh.searchWorkspace({ query: 'anything', limit: 8 });

    expect(isSearchRefusal(result)).toBe(true);
    expect((result as { message: string }).message).toContain('Tauri IPC unavailable');
    expect(fresh.lastError).toBeNull();
  });
});

/**
 * Direct port of `fleet_reduces_subagent_activity_and_tab_into_cycles_focus`
 * in `src/tui/reducer.rs` (minus Tab-into focus cycling, which is TUI-only
 * navigation with no desktop equivalent) — same scenario, same assertions, so
 * the two clients cannot silently diverge on what the roster does.
 */
describe('fleet roster reduction (ported from src/tui/reducer.rs)', () => {
  let client: MjolnrClient;

  beforeEach(() => {
    client = new MjolnrClient();
  });

  function activity(child: string, label: string) {
    client.handleUpdate({ type: 'event', sequence: 1, event: { activity: 'subagentActivity', child, label } });
  }

  it('stays hidden below two agents, shows once two are live, and clears on a fresh convocation', () => {
    expect(client.fleet).toEqual([]);

    activity('agent-a', 'started');
    expect(client.fleet).toHaveLength(1);

    activity('agent-b', 'started');
    activity('agent-a', 'deliberating');
    expect(client.fleet).toHaveLength(2);
    expect(client.fleet[0].latest).toBe('deliberating');
    expect(client.fleet[0].feed).toEqual(['started', 'deliberating']);

    activity('agent-a', 'finished');
    activity('agent-b', 'finished');
    expect(client.fleet.every((agent) => agent.done)).toBe(true);

    // A fresh convocation clears the settled roster, same as the TUI.
    activity('agent-c', 'started');
    expect(client.fleet).toHaveLength(1);
    expect(client.fleet[0].child).toBe('agent-c');
  });

  it('treats any "failed*" label as done, same as the TUI', () => {
    activity('agent-a', 'failed: timeout');
    expect(client.fleet[0].done).toBe(true);
  });
});

describe('worktree list reduction (E2)', () => {
  let client: MjolnrClient;

  beforeEach(() => {
    client = new MjolnrClient();
  });

  function spawn(child: string, branch: string, worktree: string) {
    client.handleUpdate({
      type: 'event',
      sequence: 1,
      event: {
        activity: 'subagentSpawned',
        child,
        directive: 'do a bounded thing',
        directiveTruncated: false,
        branch,
        worktree
      }
    });
  }

  function activity(child: string, label: string) {
    client.handleUpdate({ type: 'event', sequence: 1, event: { activity: 'subagentActivity', child, label } });
  }

  it('adds an entry on spawn and marks it done on a finished/failed activity label', () => {
    spawn('child-a', 'mjolnr/sub-child-a', '/work/.mjolnr/worktrees/child-a');
    expect(client.worktrees).toHaveLength(1);
    expect(client.worktrees[0].done).toBe(false);

    activity('child-a', 'working');
    expect(client.worktrees[0].done).toBe(false);

    activity('child-a', 'finished');
    expect(client.worktrees[0].done).toBe(true);
  });

  it('does not clear on a fresh convocation, unlike the fleet roster — the worktree still exists on disk', () => {
    spawn('child-a', 'mjolnr/sub-child-a', '/work/a');
    activity('child-a', 'finished');
    spawn('child-b', 'mjolnr/sub-child-b', '/work/b');

    expect(client.worktrees.map((entry) => entry.child)).toEqual(['child-a', 'child-b']);
  });
});