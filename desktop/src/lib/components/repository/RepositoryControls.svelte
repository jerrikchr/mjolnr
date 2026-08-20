<script lang="ts">
  /**
   * Operator-controlled D5 repository mutations.
   *
   * The panel is a selector: it gathers a bounded set of paths or a human
   * message, then shows the exact repository, revisions, and argv-shaped
   * values before dispatching the existing typed Rust command. The frontend
   * never runs git and never invents a successful outcome.
   */
  import { clientStore, type ClientRefusal } from '$lib/runtime/client.svelte';
  import type { ClientRepositoryState } from '$lib/runtime/contract';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Textarea } from '$lib/components/ui/textarea';

  const MAX_ACTION_PATHS = 512;

  type RepositoryAction =
    | {
        kind: 'stagePaths' | 'unstage';
        paths: string[];
        expectedIndexRevision: string | undefined;
      }
    | { kind: 'createBranch'; baseRevision: string; expectedHead: string | undefined }
    | {
        kind: 'commit';
        expectedIndexRevision: string;
      }
    | {
        kind: 'integrateChildBranch';
        expectedHead: string;
      }
    | { kind: 'fetch' }
    | { kind: 'push'; expectedHead: string }
    | { kind: 'integrateUpstream'; expectedHead: string }
    | { kind: 'rebase'; expectedHead: string }
    | { kind: 'abortRebase' };

  let open = $state(false);
  let pending = $state<RepositoryAction | null>(null);
  let branchName = $state('');
  let commitMessage = $state('');
  let integrationBranch = $state('');
  let integrationMessage = $state('');
  let rebaseTarget = $state('');
  let refusal = $state<ClientRefusal | null>(null);
  let busy = $state(false);

  let repository = $derived<ClientRepositoryState>(clientStore.snapshot.repository);
  let workspaceRoot = $derived(clientStore.snapshot.workspaceRoot ?? '(workspace root unavailable)');
  let stageablePaths = $derived(
    uniquePaths([...repository.modifiedFiles, ...repository.untrackedFiles]).filter(
      (path) => !repository.unmergedFiles.includes(path)
    )
  );
  let stagedPaths = $derived(uniquePaths(repository.stagedFiles));
  let childBranches = $derived(
    [...new Set(clientStore.worktrees.filter((entry) => entry.done).map((entry) => entry.branch))].sort()
  );

  function uniquePaths(paths: string[]): string[] {
    return [...new Set(paths)];
  }

  function capturedRepository(): boolean {
    return repository.freshness.type === 'capturedAt' && repository.trust === 'mjolnrGoverned';
  }

  function safePathSet(paths: string[]): boolean {
    return paths.length > 0 && paths.length <= MAX_ACTION_PATHS && !repository.pathsTruncated;
  }

  function canStage(): boolean {
    return (
      capturedRepository() &&
      repository.unmergedFiles.length === 0 &&
      safePathSet(stageablePaths)
    );
  }

  function canUnstage(): boolean {
    return (
      capturedRepository() &&
      repository.unmergedFiles.length === 0 &&
      safePathSet(stagedPaths)
    );
  }

  function canCreateBranch(): boolean {
    return capturedRepository() && repository.head !== undefined && repository.head !== null;
  }

  function canCommit(): boolean {
    return (
      capturedRepository() &&
      repository.unmergedFiles.length === 0 &&
      !repository.pathsTruncated &&
      stagedPaths.length > 0 &&
      repository.indexRevision !== undefined
    );
  }

  function canIntegrate(): boolean {
    return (
      capturedRepository() &&
      repository.unmergedFiles.length === 0 &&
      repository.dirtyCount === 0 &&
      repository.head !== undefined &&
      repository.head !== null &&
      childBranches.length > 0
    );
  }

  function canFetch(): boolean {
    // Fetch is inert: it touches only remote-tracking refs. It needs a captured
    // repository and nothing else — no clean tree, no staged work.
    return capturedRepository();
  }

  function canPush(): boolean {
    // Push is armed only when there is a branch checked out and the local is
    // not behind the remote (synced or ahead). A diverged or behind state is
    // refused before the network in the runtime; arming the button there would
    // only surface a refusal the human can avoid by fetching first.
    const sync = repository.remoteSync;
    return (
      capturedRepository() &&
      repository.head !== undefined &&
      repository.head !== null &&
      repository.branch !== undefined &&
      (sync.type === 'synced' || sync.type === 'ahead')
    );
  }

  function canIntegrateUpstream(): boolean {
    // The merge half of pull (fetch is the other half; the human issues both,
    // two evidenced acts rather than one compound pull). Armed only when the
    // last-seen upstream ref holds commits the local branch lacks — merging
    // otherwise is a no-op, or the runtime refuses (no upstream fetched,
    // dirty tree, conflict) and arming would only surface that refusal.
    const sync = repository.remoteSync;
    return (
      capturedRepository() &&
      repository.head !== undefined &&
      repository.head !== null &&
      repository.branch !== undefined &&
      repository.dirtyCount === 0 &&
      repository.unmergedFiles.length === 0 &&
      (sync.type === 'behind' || sync.type === 'diverged')
    );
  }

  function canRebase(): boolean {
    return (
      capturedRepository() &&
      repository.rebaseInProgress === false &&
      repository.branch !== undefined &&
      repository.head !== undefined &&
      repository.dirtyCount === 0 &&
      repository.unmergedFiles.length === 0
    );
  }

  function canAbortRebase(): boolean {
    return capturedRepository() && repository.rebaseInProgress;
  }

  function beginStage() {
    if (!canStage()) return;
    pending = {
      kind: 'stagePaths',
      paths: [...stageablePaths],
      expectedIndexRevision: repository.indexRevision
    };
    refusal = null;
    open = true;
  }

  function beginUnstage() {
    if (!canUnstage()) return;
    pending = {
      kind: 'unstage',
      paths: [...stagedPaths],
      expectedIndexRevision: repository.indexRevision
    };
    refusal = null;
    open = true;
  }

  function beginCreateBranch() {
    if (!canCreateBranch() || !repository.head) return;
    branchName = '';
    refusal = null;
    pending = { kind: 'createBranch', baseRevision: repository.head, expectedHead: repository.head };
    open = true;
  }

  function beginCommit() {
    if (!canCommit() || !repository.indexRevision) return;
    commitMessage = '';
    refusal = null;
    pending = { kind: 'commit', expectedIndexRevision: repository.indexRevision };
    open = true;
  }

  function beginIntegrate() {
    if (!canIntegrate() || !repository.head) return;
    integrationBranch = childBranches[0] ?? '';
    integrationMessage = '';
    refusal = null;
    pending = { kind: 'integrateChildBranch', expectedHead: repository.head };
    open = true;
  }

  function beginFetch() {
    if (!canFetch()) return;
    refusal = null;
    pending = { kind: 'fetch' };
    open = true;
  }

  function beginPush() {
    if (!canPush() || !repository.head) return;
    refusal = null;
    pending = { kind: 'push', expectedHead: repository.head };
    open = true;
  }

  function beginIntegrateUpstream() {
    if (!canIntegrateUpstream() || !repository.head) return;
    integrationMessage = '';
    refusal = null;
    pending = { kind: 'integrateUpstream', expectedHead: repository.head };
    open = true;
  }

  function beginRebase() {
    if (!canRebase() || !repository.head) return;
    rebaseTarget = '';
    refusal = null;
    pending = { kind: 'rebase', expectedHead: repository.head };
    open = true;
  }

  function beginAbortRebase() {
    if (!canAbortRebase()) return;
    refusal = null;
    pending = { kind: 'abortRebase' };
    open = true;
  }

  function close() {
    if (busy) return;
    open = false;
    pending = null;
    refusal = null;
  }

  function actionLabel(action: RepositoryAction): string {
    switch (action.kind) {
      case 'stagePaths':
        return 'Stage selected paths';
      case 'unstage':
        return 'Unstage selected paths';
      case 'createBranch':
        return 'Create branch';
      case 'commit':
        return 'Create commit';
      case 'integrateChildBranch':
        return 'Integrate child branch';
      case 'fetch':
        return 'Fetch from upstream';
      case 'push':
        return 'Push to upstream';
      case 'integrateUpstream':
        return 'Integrate the fetched upstream';
      case 'rebase':
        return 'Rebase current branch';
      case 'abortRebase':
        return 'Abort in-progress rebase';
    }
  }

  function baseRevision(action: RepositoryAction): string {
    switch (action.kind) {
      case 'stagePaths':
      case 'unstage':
      case 'commit':
      case 'fetch':
      case 'push':
        return repository.head ?? 'unborn';
      case 'createBranch':
        return action.baseRevision;
      case 'integrateChildBranch':
      case 'integrateUpstream':
      case 'rebase':
        return action.expectedHead;
      case 'abortRebase':
        return repository.head ?? 'unborn';
    }
  }

  function expectedIndexRevision(action: RepositoryAction): string {
    switch (action.kind) {
      case 'stagePaths':
      case 'unstage':
        return action.expectedIndexRevision ?? 'unavailable';
      case 'commit':
        return action.expectedIndexRevision;
      case 'createBranch':
      case 'integrateChildBranch':
      case 'fetch':
      case 'push':
      case 'integrateUpstream':
      case 'rebase':
      case 'abortRebase':
        return repository.indexRevision ?? 'unavailable';
    }
  }

  function pathsFor(action: RepositoryAction): string[] {
    return action.kind === 'stagePaths' || action.kind === 'unstage' ? action.paths : [];
  }

  function messageFor(action: RepositoryAction): string | undefined {
    if (action.kind === 'commit') return commitMessage.trim() || undefined;
    if (action.kind === 'integrateChildBranch' || action.kind === 'integrateUpstream') {
      return integrationMessage.trim() || undefined;
    }
    return undefined;
  }

  function actionReady(action: RepositoryAction): boolean {
    if (action.kind === 'createBranch') return branchName.trim().length > 0;
    if (action.kind === 'commit') return commitMessage.trim().length > 0;
    if (action.kind === 'integrateChildBranch') {
      return integrationBranch.trim().length > 0 && integrationMessage.trim().length > 0;
    }
    if (action.kind === 'integrateUpstream') return integrationMessage.trim().length > 0;
    if (action.kind === 'rebase') return rebaseTarget.trim().length > 0;
    return true;
  }

  function revisionStillMatches(action: RepositoryAction): boolean {
    if (action.kind === 'fetch') {
      // Fetch is inert; there is no expected revision to guard against.
      return true;
    }
    if (action.kind === 'push') {
      return repository.head === action.expectedHead;
    }
    if (action.kind === 'stagePaths' || action.kind === 'unstage') {
      return repository.indexRevision === action.expectedIndexRevision;
    }
    if (
      action.kind === 'createBranch' ||
      action.kind === 'integrateChildBranch' ||
      action.kind === 'integrateUpstream' ||
      action.kind === 'rebase'
    ) {
      return repository.head === action.expectedHead;
    }
    if (action.kind === 'abortRebase') return true;
    return repository.indexRevision === action.expectedIndexRevision;
  }

  function staleRefusal(): ClientRefusal {
    return {
      code: 'WORKSPACE_STALE_REVISION',
      message: 'The repository changed after this preview. Re-read it and review a new action.'
    };
  }

  async function confirm() {
    if (!pending || busy || !actionReady(pending)) return;
    if (!revisionStillMatches(pending)) {
      refusal = staleRefusal();
      return;
    }

    const action = pending;
    busy = true;
    refusal = null;
    let result: ClientRefusal | null;
    if (action.kind === 'stagePaths') {
      result = await clientStore.dispatch({ type: 'stagePaths', paths: action.paths });
    } else if (action.kind === 'unstage') {
      result = await clientStore.dispatch({ type: 'unstage', paths: action.paths });
    } else if (action.kind === 'createBranch') {
      result = await clientStore.dispatch({
        type: 'createBranch',
        name: branchName.trim(),
        baseRevision: action.baseRevision
      });
    } else if (action.kind === 'commit') {
      result = await clientStore.dispatch({
        type: 'commit',
        message: commitMessage.trim(),
        expectedIndexRevision: action.expectedIndexRevision
      });
    } else if (action.kind === 'integrateChildBranch') {
      result = await clientStore.dispatch({
        type: 'integrateChildBranch',
        name: integrationBranch.trim(),
        message: integrationMessage.trim(),
        expectedHead: action.expectedHead
      });
    } else if (action.kind === 'fetch') {
      result = await clientStore.dispatch({ type: 'fetch' });
    } else if (action.kind === 'push') {
      result = await clientStore.dispatch({ type: 'push', expectedHead: action.expectedHead });
    } else if (action.kind === 'integrateUpstream') {
      result = await clientStore.dispatch({
        type: 'integrateUpstream',
        message: integrationMessage.trim(),
        expectedHead: action.expectedHead
      });
    } else if (action.kind === 'rebase') {
      result = await clientStore.dispatch({
        type: 'rebase',
        onto: rebaseTarget.trim(),
        expectedHead: action.expectedHead
      });
    } else if (action.kind === 'abortRebase') {
      result = await clientStore.dispatch({ type: 'abortRebase' });
    } else {
      return;
    }
    busy = false;

    if (result) {
      refusal = result;
      return;
    }

    // The runtime re-reads and verifies the repository before acknowledging a
    // successful command. Closing the preview makes no stronger claim than
    // that acknowledgement; the next snapshot supplies the observed state.
    close();
  }
</script>

<div class="flex flex-col gap-2 border-t pt-2" data-testid="repository-controls">
  <div class="flex items-center justify-between gap-2">
    <span class="text-[0.7rem] font-medium uppercase tracking-wide text-muted-foreground">
      Controlled changes
    </span>
    <span class="text-[0.65rem] text-muted-foreground">preview → confirm</span>
  </div>

  {#if repository.freshness.type === 'capturedAt' && repository.unmergedFiles.length > 0}
    <p class="text-xs text-destructive" data-testid="repository-controls-conflict">
      Resolve {repository.unmergedFiles.length === 1 ? 'the conflict' : 'conflicts'} before staging
      or integrating.
    </p>
  {:else if repository.pathsTruncated}
    <p class="text-xs text-muted-foreground" data-testid="repository-controls-truncated">
      Controls are paused because mjolnr did not receive the complete path list.
    </p>
  {:else if repository.freshness.type !== 'capturedAt'}
    <p class="text-xs text-muted-foreground" data-testid="repository-controls-unavailable">
      Controls appear after mjolnr captures a readable repository.
    </p>
  {/if}

  {#if repository.freshness.type === 'capturedAt'}
    <div class="grid grid-cols-2 gap-1.5 pt-1">
      <Button size="sm" variant="outline" class="text-xs" disabled={!canStage()} onclick={beginStage}>
        Stage paths
      </Button>
      <Button size="sm" variant="outline" class="text-xs" disabled={!canUnstage()} onclick={beginUnstage}>
        Unstage
      </Button>
      <Button size="sm" variant="outline" class="text-xs" disabled={!canCreateBranch()} onclick={beginCreateBranch}>
        Create branch
      </Button>
      <Button size="sm" variant="outline" class="text-xs" disabled={!canCommit()} onclick={beginCommit}>
        Commit
      </Button>
      <Button size="sm" variant="outline" class="text-xs" disabled={!canFetch()} onclick={beginFetch}>
        Fetch
      </Button>
      <Button size="sm" variant="outline" class="text-xs" disabled={!canPush()} onclick={beginPush}>
        Push
      </Button>
      <Button
        class="col-span-2 text-xs"
        size="sm"
        variant="outline"
        disabled={!canIntegrateUpstream()}
        onclick={beginIntegrateUpstream}
      >
        Integrate upstream
      </Button>
      <Button
        class="col-span-2 text-xs"
        size="sm"
        variant="outline"
        disabled={!canIntegrate()}
        onclick={beginIntegrate}
      >
        Integrate settled child branch
      </Button>
      <Button
        class="col-span-2 text-xs"
        size="sm"
        variant="outline"
        disabled={!canRebase()}
        onclick={beginRebase}
      >
        Rebase current branch
      </Button>
      {#if repository.rebaseInProgress}
        <Button
          class="col-span-2 text-xs"
          size="sm"
          variant="destructive"
          disabled={!canAbortRebase()}
          onclick={beginAbortRebase}
        >
          Abort in-progress rebase
        </Button>
      {/if}
    </div>
  {/if}

  {#if childBranches.length === 0 && capturedRepository()}
    <p class="text-[0.7rem] text-muted-foreground">
      Integration stays disabled until a child branch has settled and is visible to this client.
    </p>
  {/if}
</div>

<Dialog.Root bind:open>
  <Dialog.Content class="max-h-[85vh] w-full max-w-xl overflow-y-auto">
    <Dialog.Header>
      <Dialog.Title>Review repository action</Dialog.Title>
      <Dialog.Description>
        This is an operator-controlled change. Review the exact values mjolnr will pass to the
        governed repository command before confirming.
      </Dialog.Description>
    </Dialog.Header>

    {#if pending}
      <div class="flex flex-col gap-3" data-testid="repository-action-preview">
        {#if pending.kind === 'createBranch'}
          <label class="flex flex-col gap-1 text-xs font-medium" for="repository-branch-name">
            Branch name
            <Input id="repository-branch-name" bind:value={branchName} placeholder="feature/name" />
          </label>
        {:else if pending.kind === 'commit'}
          <label class="flex flex-col gap-1 text-xs font-medium" for="repository-commit-message">
            Commit message
            <Textarea
              id="repository-commit-message"
              rows={3}
              bind:value={commitMessage}
              placeholder="Describe the staged change"
            />
          </label>
        {:else if pending.kind === 'integrateChildBranch'}
          <label class="flex flex-col gap-1 text-xs font-medium" for="repository-child-branch">
            Settled child branch
            <select
              id="repository-child-branch"
              class="border-input bg-background h-8 rounded-lg border px-2 text-sm"
              bind:value={integrationBranch}
            >
              {#each childBranches as branch}
                <option value={branch}>{branch}</option>
              {/each}
            </select>
          </label>
          <label class="flex flex-col gap-1 text-xs font-medium" for="repository-merge-message">
            Merge commit message
            <Textarea
              id="repository-merge-message"
              rows={3}
              bind:value={integrationMessage}
              placeholder="Describe the integrated child work"
            />
          </label>
        {:else if pending.kind === 'integrateUpstream'}
          <label class="flex flex-col gap-1 text-xs font-medium" for="repository-upstream-merge-message">
            Merge commit message
            <Textarea
              id="repository-upstream-merge-message"
              rows={3}
              bind:value={integrationMessage}
              placeholder="Describe the integrated upstream work"
            />
          </label>
          <p class="text-[0.7rem] text-muted-foreground">
            Used only if the merge creates a commit. When the branch is simply behind, git
            fast-forwards and consumes no message — exactly as `git pull` does.
          </p>
        {:else if pending.kind === 'rebase'}
          <label class="flex flex-col gap-1 text-xs font-medium" for="repository-rebase-target">
            Rebase onto local ref
            <Input
              id="repository-rebase-target"
              bind:value={rebaseTarget}
              placeholder="main or origin/main"
            />
          </label>
          <p class="text-[0.7rem] text-muted-foreground">
            mjolnr requires a clean tree and leaves conflicts paused for human resolution. It will
            not stash, resolve, or discard work.
          </p>
        {:else if pending.kind === 'abortRebase'}
          <p class="text-[0.7rem] text-muted-foreground">
            This clears only the observed in-progress rebase state. Files restored by git remain
            for you to inspect.
          </p>
        {/if}

        <dl class="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 rounded-md border bg-muted/20 p-3 text-xs">
          <dt class="text-muted-foreground">Operation</dt>
          <dd class="font-medium" data-testid="repository-preview-operation">{actionLabel(pending)}</dd>
          <dt class="text-muted-foreground">Repository</dt>
          <dd class="break-all font-mono" data-testid="repository-preview-root">{workspaceRoot}</dd>
          <dt class="text-muted-foreground">Base revision</dt>
          <dd class="break-all font-mono" data-testid="repository-preview-base">{baseRevision(pending)}</dd>
          <dt class="text-muted-foreground">Expected index revision</dt>
          <dd class="break-all font-mono" data-testid="repository-preview-index">{expectedIndexRevision(pending)}</dd>
          {#if pending.kind === 'createBranch'}
            <dt class="text-muted-foreground">Branch</dt>
            <dd class="break-all font-mono">{branchName.trim() || '(enter a branch name)'}</dd>
          {:else if pending.kind === 'integrateChildBranch'}
            <dt class="text-muted-foreground">Child branch</dt>
            <dd class="break-all font-mono">{integrationBranch || '(select a branch)'}</dd>
          {:else if pending.kind === 'integrateUpstream'}
            <dt class="text-muted-foreground">Merge operand</dt>
            <dd class="break-all font-mono" data-testid="repository-preview-operand">
              @{'{'}upstream{'}'} of {repository.branch ?? 'the current branch'} — the remote-tracking
              ref as last fetched, resolved by git at execution
            </dd>
          {:else if pending.kind === 'rebase'}
            <dt class="text-muted-foreground">Rebase target</dt>
            <dd class="break-all font-mono">{rebaseTarget.trim() || '(enter a local ref)'}</dd>
          {/if}
        </dl>

        <div class="flex flex-col gap-1" data-testid="repository-preview-paths">
          <span class="text-xs font-medium text-muted-foreground">Paths or hunks</span>
          {#if pathsFor(pending).length === 0}
            <span class="text-xs text-muted-foreground">No path or hunk selection; this is a branch operation.</span>
          {:else}
            <ul class="max-h-40 overflow-y-auto rounded-md border p-2 font-mono text-xs">
              {#each pathsFor(pending) as path}
                <li class="break-all">{path}</li>
              {/each}
            </ul>
          {/if}
        </div>

        {#if messageFor(pending)}
          <div class="rounded-md border bg-muted/20 p-2 text-xs">
            <span class="text-muted-foreground">Human message:</span>
            <span class="whitespace-pre-wrap">{messageFor(pending)}</span>
          </div>
        {/if}

        {#if refusal}
          <div class="flex flex-col gap-1 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-xs" data-testid="repository-action-refusal" role="alert">
            <span class="font-mono">{refusal.code ?? 'REFUSED'}</span>
            <span>{refusal.message}</span>
            {#if refusal.code === 'REPOSITORY_UNCERTAIN_EFFECT'}
              <span class="text-muted-foreground">Do not retry automatically. Re-read the repository and resolve the outcome first.</span>
            {:else}
              <span class="text-muted-foreground">No successful outcome is claimed.</span>
            {/if}
          </div>
        {/if}
      </div>
    {/if}

    <Dialog.Footer>
      <Button variant="outline" disabled={busy} onclick={close}>Cancel</Button>
      <Button
        data-testid="repository-action-confirm"
        disabled={busy || refusal?.code === 'REPOSITORY_UNCERTAIN_EFFECT' || !pending || !actionReady(pending)}
        onclick={confirm}
      >
        {busy ? 'Waiting for mjolnr…' : 'Confirm controlled change'}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
