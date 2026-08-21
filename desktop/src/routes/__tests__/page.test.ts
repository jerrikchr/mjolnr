// @vitest-environment jsdom

import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import Page from '../+page.svelte';
import { clientStore, resetClientStoreForTests } from '$lib/runtime/client.svelte';
import type { ClientDirectoryPage, ClientFileOpen, ClientSnapshot } from '$lib/runtime/contract';
import { NO_PROJECT_REPOSITORY, NO_REVIEW_THREADS } from '$lib/runtime/contract';

const { gotoMock } = vi.hoisted(() => ({
  gotoMock: vi.fn(async () => undefined)
}));

vi.mock('$app/navigation', () => ({
  goto: gotoMock
}));

const baseSnapshot: ClientSnapshot = {
  revision: 1,
  policy: 'ask',
  runActive: false,
  usage: { inputTokens: 0, outputTokens: 0 },
  budget: { providerTurns: 0, maxProviderTurns: 25, toolCalls: 0, maxToolCalls: 100 },
  messages: [],
  messagesOmitted: 0,
  recovery: { state: 'clean' },
  models: [
    { provider: 'anthropic', model: 'claude-3-5-sonnet', displayName: 'Claude 3.5 Sonnet' }
  ],
  personas: [],
  souls: [],
  routes: [],
  council: null,
  accounts: [],
  sessions: [],
  repository: NO_PROJECT_REPOSITORY,
  reviewThreads: NO_REVIEW_THREADS
};

const directoryEntry = (name: string, path: string, kind: 'directory' | 'file') => ({
  name,
  path,
  kind,
  content: { type: 'notAFile' as const },
  ignored: false,
  writable: true
});

const directoryPage = (path: string, items: ReturnType<typeof directoryEntry>[]): ClientDirectoryPage => ({
  path,
  page: 0,
  entries: { items, limit: 200, total: items.length, truncated: false },
  hasMore: false,
  trust: 'operatorControlled'
});

const editableFile = (path: string): ClientFileOpen => ({
  path,
  mode: { type: 'editable', text: `// ${path}\nfn main() {}\n`, textTruncated: false },
  digest: 'a'.repeat(64),
  sizeBytes: 24,
  writable: true,
  trust: 'operatorControlled'
});

describe('Desktop workspace route', () => {
  beforeEach(() => {
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      }
    );
    Object.defineProperty(window, 'matchMedia', {
      configurable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn()
      }))
    });
    // jsdom has no layout, so the palette primitive's scroll-into-view call
    // would throw as an unhandled rejection and fail an unrelated test.
    Element.prototype.scrollIntoView = vi.fn();
    resetClientStoreForTests();
    gotoMock.mockClear();
    clientStore.connected = true;
    clientStore.lastError = null;
    clientStore.snapshot = { ...baseSnapshot };
    vi.spyOn(clientStore, 'listDirectory').mockImplementation(async (path) => {
      if (path === '') {
        return directoryPage('', [
          directoryEntry('.mjolnr', '.mjolnr', 'directory'),
          directoryEntry('src', 'src', 'directory'),
          directoryEntry('README.md', 'README.md', 'file')
        ]);
      }
      if (path === 'src') {
        return directoryPage(path, [directoryEntry('checkout', 'src/checkout', 'directory')]);
      }
      if (path === 'src/checkout') {
        return directoryPage(path, [directoryEntry('provider.rs', 'src/checkout/provider.rs', 'file')]);
      }
      return directoryPage(path, []);
    });
    vi.spyOn(clientStore, 'openFile').mockImplementation(async (path) => editableFile(path));
  });

  // The palette renders into a portal on document.body, which survives a
  // component unmount. Without an explicit cleanup the next test finds two of
  // every control.
  afterEach(() => {
    cleanup();
    document.body.innerHTML = '';
    Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
    vi.restoreAllMocks();
  });

  it('keeps first launch, runtime choices, and workspace navigation truthful', async () => {
    const { getByTestId, getByText, queryByText, queryByRole } = render(Page);

    expect(getByTestId('launch-journey')).toBeDefined();
    expect(getByTestId('guided-setup')).toBeDefined();
    expect(getByText(/mjolnr started without a project folder/)).toBeDefined();
    expect(getByTestId('choose-workspace')).toBeDefined();
    expect(queryByText('No stored sessions are available yet for this workspace.')).toBeNull();
    expect(queryByRole('radio', { name: 'full-auto' })).toBeNull();
    expect(getByText('Claude 3.5 Sonnet')).toBeDefined();
    expect(queryByText('OpenAI')).toBeNull();

    clientStore.snapshot = { ...baseSnapshot, models: [] };
    await waitFor(() => expect(getByText('No models connected yet.')).toBeDefined());

    clientStore.snapshot = {
      ...baseSnapshot,
      session: '0190d5f0-active',
      workspaceRoot: '/test/root',
      messages: [
        { kind: 'tool', id: 'tool-1', name: 'cargo test', outcome: 'ok', detail: 'all green', detailTruncated: false }
      ]
    };

    await fireEvent.keyDown(window, { key: '3', ctrlKey: true });
    expect(getByTestId('changes-surface')).toBeDefined();

    await fireEvent.keyDown(window, { key: 'g', ctrlKey: true, shiftKey: true });
    expect(gotoMock).toHaveBeenCalledWith('/gallery');
  });

  it('treats connections as a persistent workspace surface', async () => {
    const view = render(Page);

    await fireEvent.keyDown(window, { key: '7', metaKey: true });

    expect(view.getByRole('heading', { name: 'Connections' })).toBeDefined();
    expect(view.queryByRole('dialog')).toBeNull();
    expect(view.getByPlaceholderText(/Filter connections/)).toBeDefined();
  });

  /**
   * The Repository panel is mounted in the shell, not merely written. ADR 0009
   * places it at the foot of the file explorer; the explorer is D7 and unbuilt,
   * so it lives in the sidebar footer for now. This asserts it is reachable
   * from the route at all — the component's own behaviour is covered in
   * `components/__tests__/repository.test.ts`.
   */
  it('mounts the Repository panel in the workspace shell', () => {
    const { getAllByTestId } = render(Page);

    // The Sidebar primitive renders a desktop and an off-canvas mobile copy.
    expect(getAllByTestId('repository-panel').length).toBeGreaterThan(0);
    expect(getAllByTestId('repository-no-project').length).toBeGreaterThan(0);
  });

  it('lets the no-workspace launch card open an entered path', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const view = render(Page);

    await fireEvent.input(view.getByPlaceholderText('Or enter an absolute project path'), {
      target: { value: '/tmp/project' }
    });
    await fireEvent.click(view.getByRole('button', { name: 'Open entered path' }));

    await waitFor(() =>
      expect(dispatch).toHaveBeenCalledWith({ type: 'openProject', root: '/tmp/project' })
    );
  });

  it('projects runtime workspace truth into the chooser and clears stale launch paths', async () => {
    clientStore.snapshot = { ...baseSnapshot, workspaceRoot: '/test/current-project' };
    const view = render(Page);

    await waitFor(() =>
      expect((view.getByLabelText('Project root') as HTMLInputElement).value).toBe(
        '/test/current-project'
      )
    );

    clientStore.snapshot = { ...baseSnapshot };
    await waitFor(() => expect((view.getByLabelText('Project root') as HTMLInputElement).value).toBe(''));
  });

  it('lets first launch review and dispatch a governed clone', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const view = render(Page);

    await fireEvent.input(view.getByLabelText('Repository source'), {
      target: { value: 'https://example.invalid/project.git' }
    });
    await fireEvent.input(view.getByLabelText('New project folder'), {
      target: { value: '/tmp/project' }
    });
    await fireEvent.click(view.getByRole('button', { name: 'Review clone' }));
    await fireEvent.click(view.getByTestId('clone-confirm'));

    await waitFor(() =>
      expect(dispatch).toHaveBeenCalledWith({
        type: 'cloneProject',
        source: 'https://example.invalid/project.git',
        destination: '/tmp/project'
      })
    );
  });

  it('fetches a GitHub task from the board and refreshes the projection', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    vi.spyOn(clientStore, 'queryBoard').mockResolvedValue({
      importedTasks: [],
      importedActs: [],
      frontier: [],
      fog: [],
      settled: [],
      cycles: []
    });
    const view = render(Page);
    await fireEvent.keyDown(window, { key: '3', ctrlKey: true });

    const input = view.getByLabelText('GitHub task id');
    await fireEvent.input(input, { target: { value: 'octocat/hello#42' } });
    await fireEvent.click(view.getByRole('button', { name: 'Fetch GitHub task' }));

    await waitFor(() =>
      expect(dispatch).toHaveBeenCalledWith({
        type: 'fetchTask',
        source: 'github',
        taskId: 'octocat/hello#42'
      })
    );
    expect(clientStore.queryBoard).toHaveBeenCalled();
  });

  it('fetches a bounded task batch in the selected order and source', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    vi.spyOn(clientStore, 'queryBoard').mockResolvedValue({
      importedTasks: [],
      importedActs: [],
      frontier: [],
      fog: [],
      settled: [],
      cycles: []
    });
    const view = render(Page);
    await fireEvent.keyDown(window, { key: '3', ctrlKey: true });

    await fireEvent.change(view.getByLabelText('Batch task source'), {
      target: { value: 'linear' }
    });
    await fireEvent.input(view.getByLabelText('Batch task ids'), {
      target: { value: 'SIM-42\nSIM-43' }
    });
    await fireEvent.click(view.getByRole('button', { name: 'Fetch task batch' }));

    await waitFor(() =>
      expect(dispatch).toHaveBeenCalledWith({
        type: 'fetchTasks',
        source: 'linear',
        taskIds: ['SIM-42', 'SIM-43']
      })
    );
  });

  it('submits a selected imported task with the recorded revision and repository head', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    vi.spyOn(clientStore, 'queryBoard').mockResolvedValue({
      importedTasks: [
        {
          boardId: 'item-1',
          integration: 'github',
          remoteId: 'octocat/hello#42',
          sourceUrl: 'https://github.com/octocat/hello/issues/42',
          fetchedRevision: '2026-08-06T10:00:00Z',
          title: 'Fix the parser',
          state: 'open'
        }
      ],
      importedActs: [],
      frontier: [],
      fog: [],
      settled: [],
      cycles: []
    });
    clientStore.snapshot = {
      ...baseSnapshot,
      repository: {
        ...NO_PROJECT_REPOSITORY,
        branch: 'feature-parser',
        head: 'abc123',
        freshness: { type: 'capturedAt', trigger: 'requested', sequence: 1 }
      }
    };
    const view = render(Page);
    await fireEvent.keyDown(window, { key: '3', ctrlKey: true });
    await waitFor(() => expect(view.getByText('Fix the parser')).toBeDefined());
    await fireEvent.click(view.getByRole('button', { name: 'Prepare PR' }));
    await fireEvent.input(view.getByLabelText('Pull request body'), { target: { value: 'Ready to merge.' } });
    await fireEvent.click(view.getByRole('button', { name: 'Submit pull request' }));

    await waitFor(() =>
      expect(dispatch).toHaveBeenCalledWith({
        type: 'submitChange',
        source: 'github',
        request: {
          remoteId: 'octocat/hello#42',
          expectedRevision: '2026-08-06T10:00:00Z',
          title: 'Fix the parser',
          body: 'Ready to merge.',
          headCommit: 'abc123',
          headBranch: 'feature-parser',
          baseBranch: 'main'
        }
      })
    );
  });


  /**
   * The command palette queries recorded work (Phase D4).
   *
   * Driven through the real input rather than by poking state, because the
   * debounce and the token guard are both part of what is being claimed.
   */
  describe('recorded-work search in the palette', () => {
    async function openPaletteAndType(text: string) {
      const view = render(Page);
      await fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
      const input = await waitFor(
        () => view.getAllByPlaceholderText('Type a command or search…')[0]
      );
      await fireEvent.input(input, { target: { value: text } });
      return view;
    }

    it('sends the typed query to the runtime and renders what came back', async () => {
      const search = vi.spyOn(clientStore, 'searchWorkspace').mockResolvedValue({
        items: [
          {
            sessionId: '0190d5f0-other-session',
            eventId: 'event-1',
            sequence: 42,
            matchSnippet: 'a governed tool was refused',
            occurredAt: '2026-07-30T10:00:00Z'
          }
        ],
        nextCursor: undefined
      });

      const { getAllByTestId, getAllByText } = await openPaletteAndType('refused');

      // Nothing fires on the keystroke itself: the producer's own measurement
      // says debounce rather than query per character.
      expect(search).not.toHaveBeenCalled();
      expect(getAllByTestId('search-pending').length).toBeGreaterThan(0);

      await waitFor(() => expect(search).toHaveBeenCalledTimes(1));
      expect(search).toHaveBeenCalledWith({ query: 'refused', limit: 8 });

      await waitFor(() =>
        expect(getAllByText('a governed tool was refused').length).toBeGreaterThan(0)
      );
      // The result names the session it would resume, rather than promising to
      // jump to an event — no surface can scroll a transcript to one event.
      expect(getAllByText(/resume session 0190d5f0/).length).toBeGreaterThan(0);
      search.mockRestore();
    });

    /**
     * The distinction the store went to the trouble of making. An empty page
     * says "nothing matched"; a refusal says "that could not be matched", and
     * they send a user to different remedies.
     */
    it('renders a refusal with its reason code, not as an empty result', async () => {
      const search = vi.spyOn(clientStore, 'searchWorkspace').mockResolvedValue({
        code: 'WORKSPACE_SEARCH_REFUSED',
        message: 'a search query needs at least 3 characters'
      });

      const { getAllByTestId, queryAllByTestId } = await openPaletteAndType('re');

      const refusal = await waitFor(() => getAllByTestId('search-refusal')[0]);
      expect(refusal.textContent).toContain('WORKSPACE_SEARCH_REFUSED');
      expect(refusal.textContent).toContain('at least 3 characters');
      expect(queryAllByTestId('search-empty')).toHaveLength(0);
      search.mockRestore();
    });

    it('says nothing matched, and what search does not cover, on an empty page', async () => {
      const search = vi
        .spyOn(clientStore, 'searchWorkspace')
        .mockResolvedValue({ items: [], nextCursor: undefined });

      const { getAllByTestId, queryAllByTestId } = await openPaletteAndType('nowhere');

      const empty = await waitFor(() => getAllByTestId('search-empty')[0]);
      expect(empty.textContent).toContain('Nothing in the recorded transcript matched');
      // Honest about scope: work items and review notes have no producer yet.
      expect(empty.textContent).toContain('not work items, review notes, or files on disk');
      expect(queryAllByTestId('search-refusal')).toHaveLength(0);
      search.mockRestore();
    });

    it('does not query, and shows no recorded-work group, for an empty query', async () => {
      const search = vi.spyOn(clientStore, 'searchWorkspace');
      const { queryAllByTestId } = await openPaletteAndType('');

      await waitFor(() => expect(queryAllByTestId('search-group')).toHaveLength(0));
      expect(search).not.toHaveBeenCalled();
      search.mockRestore();
    });

    /**
     * A slow early response must not repaint the list after a fast later one.
     * Without the token guard the user sees results for a query they have
     * already replaced, which is a correctness bug dressed as a flicker.
     */
    it('ignores a stale response that lands after a newer query', async () => {
      let releaseFirst: (value: unknown) => void = () => {};
      const first = new Promise((resolve) => {
        releaseFirst = resolve;
      });

      const search = vi
        .spyOn(clientStore, 'searchWorkspace')
        .mockImplementationOnce(
          async () =>
            (await first) as never
        )
        .mockResolvedValue({
          items: [
            {
              sessionId: '0190d5f0-second',
              eventId: 'event-second',
              sequence: 2,
              matchSnippet: 'second query result',
              occurredAt: '2026-07-30T10:00:00Z'
            }
          ],
          nextCursor: undefined
        });

      const { getAllByPlaceholderText, getAllByText, queryAllByText } =
        await openPaletteAndType('first');
      await waitFor(() => expect(search).toHaveBeenCalledTimes(1));

      const input = getAllByPlaceholderText('Type a command or search…')[0];
      await fireEvent.input(input, { target: { value: 'second' } });
      await waitFor(() => expect(search).toHaveBeenCalledTimes(2));
      await waitFor(() => expect(getAllByText('second query result').length).toBeGreaterThan(0));

      // The first query answers late. Its results belong to a query the user
      // has moved on from and must not appear.
      releaseFirst({
        items: [
          {
            sessionId: '0190d5f0-first',
            eventId: 'event-first',
            sequence: 1,
            matchSnippet: 'stale first result',
            occurredAt: '2026-07-30T09:00:00Z'
          }
        ],
        nextCursor: undefined
      });
      await first;

      expect(queryAllByText('stale first result')).toHaveLength(0);
      expect(getAllByText('second query result').length).toBeGreaterThan(0);
      search.mockRestore();
    });

    /**
     * A session is already open, and the runtime allows exactly one. Dispatching
     * the resume straight from the palette only produced a refusal banner, so the
     * palette releases the open session first — releasing, not ending, so the
     * session it leaves stays resumable.
     */
    it('releases the open session before resuming the result’s session', async () => {
      const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
      const search = vi.spyOn(clientStore, 'searchWorkspace').mockResolvedValue({
        items: [
          {
            sessionId: '0190d5f0-elsewhere',
            eventId: 'event-1',
            sequence: 1,
            matchSnippet: 'in another session',
            occurredAt: '2026-07-30T10:00:00Z'
          }
        ],
        nextCursor: undefined
      });

      clientStore.snapshot = { ...baseSnapshot, session: '0190d5f0-active' };
      const { getAllByText } = await openPaletteAndType('another');
      const item = await waitFor(() => getAllByText('in another session')[0]);

      // The release frees the seat, so the snapshot the follow-up reads has none.
      dispatch.mockImplementation(async (command) => {
        if (command.type === 'releaseSession') {
          clientStore.snapshot = { ...baseSnapshot, session: undefined };
        }
        return null;
      });

      await fireEvent.click(item);

      await waitFor(() => expect(dispatch).toHaveBeenCalledWith({ type: 'releaseSession' }));
      await waitFor(() =>
        expect(dispatch).toHaveBeenCalledWith({
          type: 'resumeSession',
          session: '0190d5f0-elsewhere'
        })
      );
      // Switching must never be terminal for the session being left.
      expect(dispatch).not.toHaveBeenCalledWith({ type: 'endSession' });
      dispatch.mockRestore();
      search.mockRestore();
    });
  });

  /**
   * "Open workspace" with an empty field used to swallow its own dispatch, so
   * the runtime's refusal never ran and the button looked broken. The frontend
   * no longer decides what an acceptable root is; it dispatches and shows what
   * came back.
   */
  it('always dispatches Open workspace and shows the refusal at the field', async () => {
    const dispatch = vi
      .spyOn(clientStore, 'dispatch')
      .mockResolvedValue({ code: 'PATH_OUTSIDE_WORKSPACE', message: 'a project path is required' });
    const { getAllByRole, getAllByTestId, queryAllByTestId, getAllByPlaceholderText } =
      render(Page);

    expect(queryAllByTestId('open-project-refusal')).toHaveLength(0);

    // The Sidebar primitive renders a desktop and an off-canvas mobile copy of
    // its content, so every control in it matches twice. Driving the first copy
    // is enough: both read the same reactive state.
    await fireEvent.click(getAllByRole('button', { name: /Open workspace/ })[0]);

    // Dispatched despite the empty field: the runtime owns the judgement.
    expect(dispatch).toHaveBeenCalledWith({ type: 'openProject', root: '' });

    const refusal = await waitFor(() => getAllByTestId('open-project-refusal')[0]);
    // Reason code and sentence both present. The code is the stable contract a
    // user can quote, and it keeps the notice from depending on colour alone.
    expect(refusal.textContent).toContain('PATH_OUTSIDE_WORKSPACE');
    expect(refusal.textContent).toContain('a project path is required');
    expect(refusal.getAttribute('role')).toBe('alert');

    // The field points at the refusal, so a screen reader reaches it from the
    // input rather than only by chance.
    const input = getAllByPlaceholderText('/absolute/path/to/project')[0];
    expect(input.getAttribute('aria-invalid')).toBe('true');
    expect(input.getAttribute('aria-describedby')).toBe('project-root-refusal');

    // An accepted open clears the notice rather than leaving a stale refusal
    // beside a workspace that did open.
    dispatch.mockResolvedValue(null);
    await fireEvent.click(getAllByRole('button', { name: /Open workspace/ })[0]);
    await waitFor(() => expect(queryAllByTestId('open-project-refusal')).toHaveLength(0));

    dispatch.mockRestore();
  });

  it('shows the quota window relevant to the active model, and the active persona', async () => {
    clientStore.snapshot = {
      ...baseSnapshot,
      model: 'gemini-2.5-pro',
      activePersona: 'Architect',
      quota: {
        provider: 'gemini-cli',
        windows: [
          { label: 'claude/gpt', usedFraction: 0.91, isRelevant: false },
          { label: 'gemini', usedFraction: 0.42, resetsAt: '2026-08-01T00:00:00Z', isRelevant: true }
        ]
      }
    };
    const { getByTestId, getByText } = render(Page);

    const indicator = await waitFor(() => getByTestId('quota-indicator'));
    // The relevant (gemini) window wins over the higher-used-but-irrelevant
    // claude/gpt window — same fallback order as `quota_gauge` in chrome.rs.
    expect(indicator.textContent).toContain('gemini');
    expect(indicator.textContent).toContain('42%');
    expect(indicator.textContent).not.toContain('91%');

    expect(getByText('Architect')).toBeDefined();
  });

  it('omits the quota indicator until a provider has reported quota', () => {
    const { queryByTestId, getByText } = render(Page);

    expect(queryByTestId('quota-indicator')).toBeNull();
    expect(getByText('Route default')).toBeDefined();
  });

  it('shows only connected accounts, not every provider mjolnr can authenticate', () => {
    clientStore.snapshot = {
      ...baseSnapshot,
      accounts: [
        { provider: 'anthropic', state: 'connected' },
        { provider: 'openai', state: 'needsReauth', detail: 'authentication was rejected' },
        { provider: 'ollama', state: 'disconnected' }
      ]
    };
    const { getAllByTestId } = render(Page);

    // The Sidebar primitive renders a desktop and an off-canvas mobile copy of
    // its content, so the one connected account's pill matches twice.
    const pills = getAllByTestId('account-pill');
    expect(pills.every((pill) => pill.textContent?.trim() === 'anthropic')).toBe(true);
    expect(pills.length).toBeGreaterThan(0);
  });

  it('omits the Accounts section entirely when nothing is connected', () => {
    const { queryByTestId, queryByText } = render(Page);

    expect(queryByTestId('account-pill')).toBeNull();
    expect(queryByText('Accounts')).toBeNull();
  });

  it('shows a worktree once its child is spawned, and keeps it after the child settles', async () => {
    const { getAllByTestId, queryByTestId } = render(Page);

    expect(queryByTestId('worktree-item')).toBeNull();

    clientStore.handleUpdate({
      type: 'event',
      sequence: 1,
      event: {
        activity: 'subagentSpawned',
        child: '0190d5f0-child',
        directive: 'refactor the auth module',
        directiveTruncated: false,
        branch: 'mjolnr/sub-0190d5f0-child',
        worktree: '/work/.mjolnr/worktrees/0190d5f0-child'
      }
    });

    await waitFor(() => expect(getAllByTestId('worktree-item').length).toBeGreaterThan(0));
    const items = getAllByTestId('worktree-item');
    expect(items[0].textContent).toContain('mjolnr/sub-0190d5f0-child');
    expect(items[0].textContent).toContain('/work/.mjolnr/worktrees/0190d5f0-child');

    clientStore.handleUpdate({
      type: 'event',
      sequence: 2,
      event: { activity: 'subagentActivity', child: '0190d5f0-child', label: 'finished' }
    });

    // The worktree still exists on disk after the child settles, so it stays
    // listed rather than disappearing the way a Fleet-roster row would.
    await waitFor(() => expect(getAllByTestId('worktree-item').length).toBeGreaterThan(0));
  });

  it('opens the governance modal on ⌘G, showing real routes and an honest empty Council tab', async () => {
    clientStore.snapshot = {
      ...baseSnapshot,
      activePersona: 'Architect',
      personas: [{ name: 'Architect', scope: 'project' }, { name: 'Minimalist', scope: 'user' }],
      routes: [
        { name: 'default', roles: ['default'], provider: 'anthropic', model: 'claude-sonnet-5' }
      ]
    };
    const { getAllByText, getByRole } = render(Page);

    await fireEvent.keyDown(window, { key: 'g', ctrlKey: true });
    const dialog = await waitFor(() => getByRole('dialog'));

    // Council is the default tab, and nothing is running — an honest empty
    // state, never a fabricated quorum figure.
    expect(dialog.textContent).toContain('No completed council review or live convocation is present');

    await fireEvent.click(getAllByText('Model & Role Routes')[0]);
    await waitFor(() => expect(dialog.textContent).toContain('claude-sonnet-5'));

    await fireEvent.click(getAllByText('SOUL.md & Personas')[0]);
    await waitFor(() => expect(getAllByText('Minimalist').length).toBeGreaterThan(0));
  });

  it('opens the governance modal to the Soul tab from the sidebar Persona row', async () => {
    clientStore.snapshot = { ...baseSnapshot, activePersona: 'Architect' };
    const { getAllByText, getByRole } = render(Page);

    await fireEvent.click(getAllByText('Architect')[0]);
    const dialog = await waitFor(() => getByRole('dialog'));
    expect(dialog.textContent).toContain('Active persona');
  });

  it('renders a completed council distribution without turning it into a vote', async () => {
    clientStore.snapshot = {
      ...baseSnapshot,
      council: {
        reviewId: 'review-1',
        question: 'Which boundary should land first?',
        roundsConducted: 2,
        artifact: { path: 'docs/plan.md', sourceDigest: 'abcdef1234567890' },
        contributions: [
          {
            role: 'plan',
            proposal: 'Keep the gate in Rust.',
            critique: 'The client must not approve it.'
          }
        ],
        findings: [
          {
            id: 'finding-1',
            section: 'Question',
            title: 'Council recommendation',
            positions: [
              {
                role: 'plan',
                response: 'Keep the gate in Rust, with evidence.',
                critique: 'The client must not approve it.'
              }
            ]
          }
        ]
      }
    };
    const { getByRole, getByText } = render(Page);

    await fireEvent.keyDown(window, { key: 'g', ctrlKey: true });
    const dialog = await waitFor(() => getByRole('dialog'));

    expect(getByText('Which boundary should land first?')).toBeDefined();
    expect(getByText('Keep the gate in Rust.')).toBeDefined();
    expect(getByText('The client must not approve it.')).toBeDefined();
    expect(dialog.textContent).toContain('Artifact: docs/plan.md');
    expect(dialog.textContent).toContain('Section: Question');
    expect(dialog.textContent).toContain('Advisory only');
    expect(dialog.textContent).not.toContain('quorum');
  });

  it('composes an amendment only from accepted findings, and opens it as unsaved editor text', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const councilBase = {
      reviewId: 'review-1',
      question: 'docs/plan.md',
      roundsConducted: 1,
      artifact: { path: 'docs/plan.md', sourceDigest: 'abcdef1234567890' },
      contributions: [],
      findings: [
        {
          id: 'finding-1',
          section: 'Goal',
          title: 'Tighten the goal',
          positions: [{ role: 'plan', response: 'Say what ships.', critique: null }]
        }
      ]
    };
    clientStore.snapshot = { ...baseSnapshot, council: councilBase };
    const { getByRole, getByText, queryByText, getByTestId } = render(Page);

    await fireEvent.keyDown(window, { key: 'g', ctrlKey: true });
    const dialog = await waitFor(() => getByRole('dialog'));

    // Nothing accepted: composing is refused at the surface, and the reason
    // says so rather than leaving a dead button.
    expect(getByRole('button', { name: 'Compose amended artifact' })).toHaveProperty(
      'disabled',
      true
    );
    expect(dialog.textContent).toContain('Accept at least one finding first');
    expect(queryByText('Open draft in editor')).toBeNull();

    // Accepting one finding funds the composition.
    clientStore.snapshot = {
      ...baseSnapshot,
      council: {
        ...councilBase,
        findings: [
          {
            ...councilBase.findings[0],
            disposition: { disposition: 'accept', note: null, decidedAt: '2026-08-04T00:00:00Z' }
          }
        ]
      }
    };
    await waitFor(() =>
      expect(getByRole('button', { name: 'Compose amended artifact' })).toHaveProperty(
        'disabled',
        false
      )
    );
    await fireEvent.click(getByRole('button', { name: 'Compose amended artifact' }));
    expect(dispatch).toHaveBeenCalledWith({
      type: 'proposeCouncilAmendment',
      reviewId: 'review-1'
    });

    // The composed draft arrives on the snapshot, never as a write.
    clientStore.snapshot = {
      ...baseSnapshot,
      council: {
        ...councilBase,
        amendment: {
          reviewId: 'review-1',
          path: 'docs/plan.md',
          sourceDigest: 'abcdef1234567890',
          acceptedFindings: 1,
          text: '# Goal\nship it\n> **Accepted finding — Tighten the goal**\n'
        }
      }
    };
    await waitFor(() => expect(getByText('Open draft in editor')).toBeDefined());
    await fireEvent.click(getByText('Open draft in editor'));

    // It lands in the editor as text, so saving it is the ordinary governed
    // save the human performs — not something the council did.
    const editor = await waitFor(() => getByTestId('editor-pane'));
    expect(editor.textContent).toContain('docs/plan.md');
    expect(dispatch).not.toHaveBeenCalledWith(expect.objectContaining({ type: 'saveFile' }));
  });

  /**
   * The D7 mock panes: the explorer rail and the terminal both open from the
   * header or their shortcut, and both close the same way. Visual-only —
   * nothing here claims the runtime scanned a repository or ran a command.
   */
  it('toggles the file explorer and terminal from header buttons and shortcuts', async () => {
    const { queryByTestId, getByTestId, getByRole } = render(Page);

    expect(queryByTestId('file-explorer')).toBeNull();
    expect(queryByTestId('terminal-pane')).toBeNull();
    expect(queryByTestId('inspector-pane')).toBeNull();

    await fireEvent.keyDown(window, { key: 'i', ctrlKey: true });
    expect(getByTestId('inspector-pane')).toBeDefined();
    await fireEvent.click(getByRole('button', { name: 'Close Inspector' }));
    expect(queryByTestId('inspector-pane')).toBeNull();

    await fireEvent.keyDown(window, { key: 'e', ctrlKey: true });
    expect(getByTestId('file-explorer')).toBeDefined();
    expect(queryByTestId('terminal-pane')).toBeNull();

    await fireEvent.keyDown(window, { key: '\\', ctrlKey: true });
    expect(getByTestId('terminal-pane')).toBeDefined();

    // The header buttons mirror the same state the shortcuts toggle.
    await fireEvent.click(getByRole('button', { name: 'Toggle file explorer (⌘E)' }));
    expect(queryByTestId('file-explorer')).toBeNull();

    await fireEvent.keyDown(window, { key: '\\', ctrlKey: true });
    expect(queryByTestId('terminal-pane')).toBeNull();
  });

  it('opens editor tabs, preserves the active tab, and closes each tab independently', async () => {
    const { queryByTestId, getByTestId, getByText, getByRole } = render(Page);

    await fireEvent.keyDown(window, { key: 'e', ctrlKey: true });
    await waitFor(() => expect(getByText('src')).toBeDefined());
    await fireEvent.click(getByText('src'));
    await waitFor(() => expect(getByText('checkout')).toBeDefined());
    await fireEvent.click(getByText('checkout'));
    await waitFor(() => expect(getByText('provider.rs')).toBeDefined());
    await fireEvent.click(getByText('provider.rs'));

    await waitFor(() => expect(getByTestId('code-editor')).toBeDefined());
    expect(getByTestId('editor-status').textContent).toContain('⌘S save');
    const activeTab = getByRole('tab', { name: 'Open src/checkout/provider.rs' });
    expect(activeTab.getAttribute('aria-selected')).toBe('true');
    expect(activeTab.getAttribute('aria-controls')).toBe('editor-panel-src-checkout-provider-rs');
    expect(getByTestId('code-editor').getAttribute('aria-keyshortcuts')).toContain('Control+W');
    const providerContent = getByTestId('code-editor').querySelector('.cm-content') as HTMLElement;
    providerContent.textContent = `${providerContent.textContent}\n// unsaved tab edit`;
    await fireEvent.input(providerContent);

    await fireEvent.click(getByText('README.md'));
    await waitFor(() => expect(getByTestId('code-editor')).toBeDefined());
    expect(getByTestId('editor-tab-README.md')).toBeDefined();
    expect(getByTestId('editor-tab-src/checkout/provider.rs')).toBeDefined();

    await fireEvent.click(getByRole('tab', { name: 'Open src/checkout/provider.rs' }));
    expect(getByTestId('editor-status').textContent).toContain('⌘S save');
    expect(getByTestId('code-editor').textContent).toContain('// unsaved tab edit');

    await fireEvent.click(getByRole('button', { name: 'Close README.md' }));
    expect(queryByTestId('editor-tab-README.md')).toBeNull();
    expect(getByTestId('editor-pane')).toBeDefined();

    await fireEvent.click(getByRole('button', { name: 'Close src/checkout/provider.rs' }));
    expect(queryByTestId('editor-pane')).toBeNull();
  });

  it('shows a stale-file refusal in the editor without claiming the edit was saved', async () => {
    const saveFile = vi.spyOn(clientStore, 'saveFile').mockResolvedValue({
      code: 'STALE_FILE_VERSION',
      message: 'the file changed outside mjolnr; reload it before saving'
    });
    const { getByTestId, getByText, getByRole } = render(Page);

    await fireEvent.keyDown(window, { key: 'e', ctrlKey: true });
    await fireEvent.click(getByText('src'));
    await fireEvent.click(getByText('checkout'));
    await fireEvent.click(getByText('provider.rs'));
    await waitFor(() => expect(getByTestId('code-editor')).toBeDefined());

    const content = getByTestId('code-editor').querySelector('.cm-content') as HTMLElement;
    content.textContent = `${content.textContent}\n// changed outside mjolnr`;
    await fireEvent.input(content);
    await fireEvent.keyDown(content, { key: 's', ctrlKey: true });

    const refusal = await waitFor(() => getByRole('alert'));
    expect(refusal.textContent).toContain('the file changed outside mjolnr');
    expect(getByTestId('editor-status').textContent).toContain('save refused');
    expect(refusal.textContent?.toLowerCase()).not.toContain('saved');
    expect(saveFile).toHaveBeenCalledWith(
      'src/checkout/provider.rs',
      'a'.repeat(64),
      expect.stringContaining('// changed outside mjolnr')
    );
    saveFile.mockRestore();
  });

  it('persists the human autosave preference through the desktop bridge', async () => {
    const savePreferences = vi
      .spyOn(clientStore, 'saveEditorPreferences')
      .mockResolvedValue(null);
    const { getByTestId, getByText } = render(Page);

    await fireEvent.keyDown(window, { key: 'e', ctrlKey: true });
    await fireEvent.click(getByText('src'));
    await fireEvent.click(getByText('checkout'));
    await fireEvent.click(getByText('provider.rs'));
    await waitFor(() => expect(getByTestId('editor-autosave-toggle')).toBeDefined());

    const toggle = getByTestId('editor-autosave-toggle') as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    await fireEvent.click(toggle);

    await waitFor(() => expect(savePreferences).toHaveBeenCalledWith({ autosave: true }));
    expect(toggle.checked).toBe(true);
    savePreferences.mockRestore();
  });

  it('finds a file beyond the currently expanded explorer folders', async () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} });
    vi.spyOn(clientStore, 'searchWorkspace').mockResolvedValue({ items: [] });
    const { getByPlaceholderText, getByText, getByTestId } = render(Page);

    await fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    const input = getByPlaceholderText('Type a command or search…');
    await fireEvent.input(input, { target: { value: 'provider.rs' } });

    await waitFor(() => expect(getByTestId('file-search-group')).toBeDefined());
    await waitFor(() => expect(getByText('src/checkout/provider.rs')).toBeDefined());
    await fireEvent.click(getByText('src/checkout/provider.rs'));
    await waitFor(() => expect(getByTestId('code-editor')).toBeDefined());
  });

  it('collapses and expands folders in the explorer tree', async () => {
    const { getAllByText, queryByText, getByTestId } = render(Page);

    await fireEvent.keyDown(window, { key: 'e', ctrlKey: true });
    expect(getByTestId('file-explorer')).toBeDefined();
    await waitFor(() => expect(getAllByText('src').length).toBeGreaterThan(0));
    await fireEvent.click(getAllByText('src')[0]);
    await waitFor(() => expect(getAllByText('checkout').length).toBeGreaterThan(0));
    await fireEvent.click(getAllByText('checkout')[0]);
    await waitFor(() => expect(getAllByText('provider.rs').length).toBeGreaterThan(0));

    await fireEvent.click(getAllByText('checkout')[0]);
    expect(queryByText('provider.rs')).toBeNull();

    await fireEvent.click(getAllByText('checkout')[0]);
    await waitFor(() => expect(getAllByText('provider.rs').length).toBeGreaterThan(0));
  });

  it('offers explorer and terminal toggles in the command palette', async () => {
    const { getAllByText } = render(Page);

    await fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    await waitFor(() =>
      expect(getAllByText('Toggle file explorer').length).toBeGreaterThan(0)
    );
    expect(getAllByText('Toggle terminal').length).toBeGreaterThan(0);
    expect(getAllByText('Open code graph').length).toBeGreaterThan(0);
  });

  it('opens the graph surface and reports browser-mode refusal honestly', async () => {
    const { getAllByText, getByTestId, getByRole } = render(Page);

    await fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    await waitFor(() => expect(getAllByText('Open code graph').length).toBeGreaterThan(0));
    await fireEvent.click(getAllByText('Open code graph')[0]);

    expect(getByTestId('graph-pane')).toBeDefined();
    await waitFor(() => expect(getByRole('alert').textContent).toContain('Tauri IPC unavailable'));
  });
});
