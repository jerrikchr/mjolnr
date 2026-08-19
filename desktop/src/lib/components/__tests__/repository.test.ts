// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, cleanup } from '@testing-library/svelte';
import RepositoryPanel from '../repository/RepositoryPanel.svelte';
import { clientStore, resetClientStoreForTests } from '$lib/runtime/client.svelte';
import type { ClientRepositoryState, ClientSnapshot } from '$lib/runtime/contract';
import { NO_PROJECT_REPOSITORY, NO_REVIEW_THREADS } from '$lib/runtime/contract';

const baseSnapshot: ClientSnapshot = {
  revision: 3,
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

/** A read repository, clean and level with its upstream — the mockup's cell. */
const readRepository: ClientRepositoryState = {
  branch: 'main',
  head: 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678',
  indexRevision: 'idx-1',
  dirtyCount: 0,
  dirtyCountTruncated: false,
  stagedFiles: [],
  modifiedFiles: [],
  untrackedFiles: [],
  unmergedFiles: [],
  rebaseInProgress: false,
  pathsTruncated: false,
  remoteSync: { type: 'synced' },
  freshness: { type: 'capturedAt', trigger: 'projectOpened', sequence: 12 },
  trust: 'smedGoverned'
};

function mount(repository: ClientRepositoryState) {
  clientStore.snapshot = { ...baseSnapshot, repository };
  return render(RepositoryPanel);
}

describe('Repository panel (D5 UI, ADR 0009 layout)', () => {
  beforeEach(() => {
    resetClientStoreForTests();
  });

  afterEach(() => {
    resetClientStoreForTests();
    cleanup();
  });

  it('renders the branch, head, dirty, staged, and sync rows ADR 0009 specifies', () => {
    const { getByTestId } = mount({
      ...readRepository,
      dirtyCount: 3,
      stagedFiles: ['src/a.rs', 'src/b.rs'],
      modifiedFiles: ['src/a.rs', 'src/b.rs', 'src/c.rs']
    });

    expect(getByTestId('repository-branch').textContent).toContain('main');
    // Abbreviated, but from the full object id the runtime sent — the panel
    // never receives a pre-shortened head it could not lengthen again.
    expect(getByTestId('repository-head').textContent).toContain('a1b2c3d');
    expect(getByTestId('repository-dirty').textContent).toContain('3');
    expect(getByTestId('repository-staged').textContent).toContain('2');
    expect(getByTestId('repository-sync')).toBeDefined();
  });

  /**
   * ADR 0009 trap 1. The mockup reads `sync: synced` in the verified colour;
   * ADR 0008 forbids both halves. This asserts on the words, because the whole
   * failure mode is a surface that shortens the sentence back to one word.
   */
  it('never renders a bare "synced", and always qualifies the position as of a past read', () => {
    const { getByTestId } = mount(readRepository);

    const sync = getByTestId('repository-sync');
    expect(sync.textContent).toContain('Level with the ref last seen');
    expect(sync.textContent).not.toMatch(/\bsynced\b/i);
    // The qualifier is present with no `remoteSyncAsOf` at all: a fresh clone
    // writes its tracking ref without a reflog entry, so absence is ordinary
    // and must not be allowed to drop the sentence (ADR 0008).
    expect(getByTestId('repository-sync-as-of').textContent).toContain(
      'the last time smed saw the remote'
    );
    expect(getByTestId('repository-sync-as-of').textContent).toContain('fetch');
  });

  it('sharpens the qualifier with a timestamp when one exists, without replacing it', () => {
    const { getByTestId } = mount({
      ...readRepository,
      remoteSync: { type: 'diverged', ahead: 2, behind: 5 },
      remoteSyncAsOf: '2026-07-30T11:02:00Z'
    });

    const sync = getByTestId('repository-sync');
    expect(sync.textContent).toContain('2 ahead, 5 behind the ref last seen');
    expect(getByTestId('repository-sync-as-of').textContent).toContain('2026-07-30T11:02:00Z');
    expect(getByTestId('repository-sync-as-of').textContent).toContain(
      'the last time smed saw the remote'
    );
  });

  it('reports ahead and behind counts against the ref, singular and plural', () => {
    const ahead = mount({ ...readRepository, remoteSync: { type: 'ahead', count: 1 } });
    expect(ahead.getByTestId('repository-sync').textContent).toContain(
      '1 commit ahead of the ref last seen'
    );
    cleanup();

    const behind = mount({ ...readRepository, remoteSync: { type: 'behind', count: 4 } });
    expect(behind.getByTestId('repository-sync').textContent).toContain(
      '4 commits behind the ref last seen'
    );
  });

  /**
   * `unknown` means no upstream or git would not answer. It has meant "we did
   * not look" for the whole life of the field, and ADR 0008 retired that
   * meaning — so the row says which one it is rather than leaving a reader to
   * assume the old one.
   */
  it('says what unknown sync means, and does not claim the position as of anything', () => {
    const { getByTestId, queryByTestId } = mount({
      ...readRepository,
      remoteSync: { type: 'unknown' }
    });

    const sync = getByTestId('repository-sync');
    expect(sync.textContent).toContain('No upstream to compare against');
    expect(sync.textContent).toContain('not "smed did not look"');
    expect(queryByTestId('repository-sync-as-of')).toBeNull();
  });

  /**
   * ADR 0009 trap 2. The mockup shows the rows with no capture marker, which
   * turns a reading into a claim about now.
   */
  it('always says when the projection was captured, never that it is live', () => {
    const { getByTestId } = mount(readRepository);

    const marker = getByTestId('repository-freshness');
    expect(marker.textContent).toContain('the project was opened');
    expect(marker.textContent).toContain('#12');
    expect(marker.textContent).toContain('not what it would say now');
  });

  it('names each refresh trigger, and falls back to the wire value it does not know', () => {
    const written = mount({
      ...readRepository,
      freshness: { type: 'capturedAt', trigger: 'toolWrite', sequence: 4 }
    });
    expect(written.getByTestId('repository-freshness').textContent).toContain(
      'a governed tool wrote to the workspace'
    );
    cleanup();

    // A trigger this build does not know about is rendered verbatim. A value
    // the runtime sent is better evidence than "unknown".
    const future = mount({
      ...readRepository,
      freshness: { type: 'capturedAt', trigger: 'someFutureTrigger' as never, sequence: 9 }
    });
    expect(future.getByTestId('repository-freshness').textContent).toContain('someFutureTrigger');
  });

  it('tells "no project" apart from "git would not answer"', () => {
    const none = mount(NO_PROJECT_REPOSITORY);
    expect(none.getByTestId('repository-no-project')).toBeDefined();
    expect(none.queryByTestId('repository-branch')).toBeNull();
    // Nothing to re-read, so the control is not offered as if there were.
    expect(
      none.getByRole('button', { name: 'Re-read repository state' }).hasAttribute('disabled')
    ).toBe(true);
    cleanup();

    const broken = mount({
      ...NO_PROJECT_REPOSITORY,
      freshness: {
        type: 'unavailable',
        code: 'REPOSITORY_UNCERTAIN_EFFECT',
        detail: 'git exited 128'
      }
    });
    const unavailable = broken.getByTestId('repository-unavailable');
    // The reason code rides alongside the sentence: it is the stable contract
    // a user can quote, matching every other refusal in this client.
    expect(unavailable.textContent).toContain('REPOSITORY_UNCERTAIN_EFFECT');
    expect(unavailable.textContent).toContain('git exited 128');
    expect(broken.queryByTestId('repository-branch')).toBeNull();
  });

  it('marks a truncated count and a truncated path list rather than reporting them as totals', () => {
    const { getByTestId } = mount({
      ...readRepository,
      dirtyCount: 500,
      dirtyCountTruncated: true,
      stagedFiles: ['a', 'b'],
      pathsTruncated: true
    });

    expect(getByTestId('repository-dirty').textContent).toContain('500+');
    expect(getByTestId('repository-dirty').textContent).toContain("smed's bound");
    expect(getByTestId('repository-staged').textContent).toContain('2+');
  });

  it('surfaces unmerged paths as conflicts rather than as ordinary stageable changes', () => {
    const { getByTestId } = mount({
      ...readRepository,
      unmergedFiles: ['src/conflicted.rs'],
      dirtyCount: 1
    });

    const unmerged = getByTestId('repository-unmerged');
    expect(unmerged.textContent).toContain('1 path unmerged');
    expect(unmerged.textContent).toContain('resolve before staging');
  });

  /**
   * The panel selects; it does not act (AGENTS.md §11.3). Re-reading is the one
   * thing it dispatches, and `refreshRepository` carries no root — the runtime
   * reads the project it already has open.
   */
  it('re-reads through the runtime and never through a second path', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const { getByRole } = mount(readRepository);

    await fireEvent.click(getByRole('button', { name: 'Re-read repository state' }));

    expect(dispatch).toHaveBeenCalledWith({ type: 'refreshRepository' });
    dispatch.mockRestore();
  });

  it('shows the runtime-owned trust class rather than assuming one', () => {
    const { getByText } = mount(readRepository);
    expect(getByText('smedGoverned')).toBeDefined();
  });

  it('loads bounded recent history through the client query', async () => {
    const query = vi.spyOn(clientStore, 'queryRepositoryHistory').mockResolvedValue({
      entries: [
        {
          revision: 'abc123456789',
          author: 'smed Test',
          authoredAt: '2026-08-10T12:00:00Z',
          subject: 'Add governed history'
        }
      ],
      hasMore: true,
      limit: 20,
      trust: 'smedGoverned'
    });
    const view = mount(readRepository);

    await fireEvent.click(view.getByRole('button', { name: 'Show history' }));

    expect(query).toHaveBeenCalledWith(20);
    expect(view.getByTestId('repository-history').textContent).toContain('Add governed history');
    expect(view.getByTestId('repository-history').textContent).toContain('Showing the newest 20');
  });
});
