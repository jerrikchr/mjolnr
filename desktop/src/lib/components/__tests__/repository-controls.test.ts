// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import RepositoryControls from '../repository/RepositoryControls.svelte';
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

const repository: ClientRepositoryState = {
  branch: 'main',
  head: 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678',
  indexRevision: 'idx-1',
  dirtyCount: 2,
  dirtyCountTruncated: false,
  stagedFiles: [],
  modifiedFiles: ['src/a.rs'],
  untrackedFiles: ['notes.txt'],
  unmergedFiles: [],
  rebaseInProgress: false,
  pathsTruncated: false,
  remoteSync: { type: 'unknown' },
  freshness: { type: 'capturedAt', trigger: 'requested', sequence: 12 },
  trust: 'smedGoverned'
};

function mount(overrides: Partial<ClientRepositoryState> = {}) {
  clientStore.snapshot = {
    ...baseSnapshot,
    workspaceRoot: 'C:/work/smed',
    repository: { ...repository, ...overrides }
  };
  return render(RepositoryControls);
}

describe('Repository controls (D5 preview boundary)', () => {
  beforeEach(() => resetClientStoreForTests());

  afterEach(async () => {
    cleanup();
    // bits-ui restores body scroll state on a short delayed cleanup after a
    // dialog closes. Let that timer settle before Vitest tears down jsdom.
    await new Promise((resolve) => setTimeout(resolve, 40));
    vi.restoreAllMocks();
    resetClientStoreForTests();
  });

  it('previews the exact repository, revisions, and paths before staging', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const view = mount();

    await fireEvent.click(view.getByRole('button', { name: 'Stage paths' }));

    expect(view.getByTestId('repository-preview-root').textContent).toContain('C:/work/smed');
    expect(view.getByTestId('repository-preview-base').textContent).toContain('a1b2c3d');
    expect(view.getByTestId('repository-preview-index').textContent).toContain('idx-1');
    expect(view.getByTestId('repository-preview-paths').textContent).toContain('src/a.rs');
    expect(view.getByTestId('repository-preview-paths').textContent).toContain('notes.txt');

    await fireEvent.click(view.getByTestId('repository-action-confirm'));
    expect(dispatch).toHaveBeenCalledWith({
      type: 'stagePaths',
      paths: ['src/a.rs', 'notes.txt']
    });
  });

  it('keeps the human commit message in the preview and typed command', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const view = mount({ stagedFiles: ['src/a.rs'], dirtyCount: 1 });

    await fireEvent.click(view.getByRole('button', { name: 'Commit' }));
    await fireEvent.input(view.getByLabelText('Commit message'), {
      target: { value: 'Add the governed repository controls' }
    });

    expect(view.getByTestId('repository-action-preview').textContent).toContain(
      'Add the governed repository controls'
    );
    await fireEvent.click(view.getByTestId('repository-action-confirm'));

    expect(dispatch).toHaveBeenCalledWith({
      type: 'commit',
      message: 'Add the governed repository controls',
      expectedIndexRevision: 'idx-1'
    });
  });

  it('previews branch creation from the observed head', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const view = mount({ dirtyCount: 0 });

    await fireEvent.click(view.getByRole('button', { name: 'Create branch' }));
    await fireEvent.input(view.getByLabelText('Branch name'), {
      target: { value: 'feature/d5-controls' }
    });
    await fireEvent.click(view.getByTestId('repository-action-confirm'));

    expect(dispatch).toHaveBeenCalledWith({
      type: 'createBranch',
      name: 'feature/d5-controls',
      baseRevision: repository.head
    });
  });

  it('offers only settled child branches for integration and requires a message', async () => {
    clientStore.worktrees = [
      {
        child: 'child-1',
        branch: 'smed/child-1',
        path: 'C:/work/child-1',
        directive: 'finish the change',
        done: true
      },
      {
        child: 'child-2',
        branch: 'smed/child-2',
        path: 'C:/work/child-2',
        directive: 'still running',
        done: false
      }
    ];
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const view = mount({ dirtyCount: 0 });

    await fireEvent.click(view.getByRole('button', { name: 'Integrate settled child branch' }));
    expect(view.getByRole('option', { name: 'smed/child-1' })).toBeDefined();
    expect(view.queryByRole('option', { name: 'smed/child-2' })).toBeNull();

    await fireEvent.input(view.getByLabelText('Merge commit message'), {
      target: { value: 'Integrate the settled child work' }
    });
    await fireEvent.click(view.getByTestId('repository-action-confirm'));

    expect(dispatch).toHaveBeenCalledWith({
      type: 'integrateChildBranch',
      name: 'smed/child-1',
      message: 'Integrate the settled child work',
      expectedHead: repository.head
    });
  });

  it('does not offer staging controls for a conflicted repository', () => {
    const view = mount({
      unmergedFiles: ['src/conflicted.rs'],
      modifiedFiles: ['src/conflicted.rs', 'src/other.rs'],
      dirtyCount: 2
    });

    expect(view.getByTestId('repository-controls-conflict').textContent).toContain('Resolve');
    expect(view.getByRole('button', { name: 'Stage paths' }).hasAttribute('disabled')).toBe(true);
    expect(view.getByRole('button', { name: 'Commit' }).hasAttribute('disabled')).toBe(true);
  });

  it('integrates a fetched upstream with the human message and the observed head', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const view = mount({ dirtyCount: 0, remoteSync: { type: 'diverged', ahead: 1, behind: 2 } });

    await fireEvent.click(view.getByRole('button', { name: 'Integrate upstream' }));

    // The merge operand is the fetched tracking ref, resolved by git — never
    // a revision the client invents.
    expect(view.getByTestId('repository-preview-operand').textContent).toContain('@{upstream}');
    expect(view.getByTestId('repository-preview-base').textContent).toContain(repository.head ?? '');

    await fireEvent.click(view.getByTestId('repository-action-confirm'));
    expect(dispatch).not.toHaveBeenCalled();

    await fireEvent.input(view.getByLabelText('Merge commit message'), {
      target: { value: 'Take the fetched upstream after review' }
    });
    await fireEvent.click(view.getByTestId('repository-action-confirm'));

    expect(dispatch).toHaveBeenCalledWith({
      type: 'integrateUpstream',
      message: 'Take the fetched upstream after review',
      expectedHead: repository.head
    });
  });

  it('arms upstream integration only when the last-seen upstream holds unknown commits', () => {
    const behind = mount({ dirtyCount: 0, remoteSync: { type: 'behind', count: 2 } });
    expect(
      behind.getByRole('button', { name: 'Integrate upstream' }).hasAttribute('disabled')
    ).toBe(false);
    cleanup();

    const synced = mount({ dirtyCount: 0, remoteSync: { type: 'synced' } });
    expect(
      synced.getByRole('button', { name: 'Integrate upstream' }).hasAttribute('disabled')
    ).toBe(true);
    cleanup();

    const dirty = mount({
      dirtyCount: 1,
      modifiedFiles: ['src/a.rs'],
      remoteSync: { type: 'diverged', ahead: 1, behind: 1 }
    });
    expect(
      dirty.getByRole('button', { name: 'Integrate upstream' }).hasAttribute('disabled')
    ).toBe(true);
  });

  it('refuses an upstream merge preview whose head moved before confirmation', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const view = mount({ dirtyCount: 0, remoteSync: { type: 'behind', count: 1 } });

    await fireEvent.click(view.getByRole('button', { name: 'Integrate upstream' }));
    await fireEvent.input(view.getByLabelText('Merge commit message'), {
      target: { value: 'Must not merge onto a moved head' }
    });
    clientStore.snapshot = {
      ...clientStore.snapshot,
      repository: { ...clientStore.snapshot.repository, head: 'ffffffffffffffffffffffffffffffffffffffff' }
    };
    await fireEvent.click(view.getByTestId('repository-action-confirm'));

    expect(dispatch).not.toHaveBeenCalled();
    expect(view.getByTestId('repository-action-refusal').textContent).toContain(
      'WORKSPACE_STALE_REVISION'
    );
  });

  it('previews a rebase target and pins the observed head', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const view = mount({ dirtyCount: 0 });

    await fireEvent.click(view.getByRole('button', { name: 'Rebase current branch' }));
    await fireEvent.input(view.getByLabelText('Rebase onto local ref'), {
      target: { value: 'main' }
    });
    expect(view.getByTestId('repository-action-preview').textContent).toContain('main');
    await fireEvent.click(view.getByTestId('repository-action-confirm'));

    expect(dispatch).toHaveBeenCalledWith({
      type: 'rebase',
      onto: 'main',
      expectedHead: repository.head
    });
  });

  it('offers abort only for an observed in-progress rebase', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    const view = mount({ rebaseInProgress: true, dirtyCount: 1, unmergedFiles: ['src/a.rs'] });

    await fireEvent.click(view.getByRole('button', { name: 'Abort in-progress rebase' }));
    await fireEvent.click(view.getByTestId('repository-action-confirm'));

    expect(dispatch).toHaveBeenCalledWith({ type: 'abortRebase' });
  });
});
