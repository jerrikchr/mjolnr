<script lang="ts">
  import { onMount } from 'svelte';
  import { clientStore, type ClientRefusal } from '$lib/runtime/client.svelte';
  import type { ClientBoardNode, ClientBoardOverview, ClientImportedAct, ClientImportedTask } from '$lib/runtime/contract';
  import { Badge } from '$lib/components/ui/badge';
  import * as Empty from '$lib/components/ui/empty';

  function provenanceLabel(node: ClientBoardNode): string {
    switch (node.provenance) {
      case 'smedGoverned':
        return 'smed-governed';
      case 'operatorControlled':
        return 'Operator';
      case 'externalUnverified':
        return 'External';
    }
  }

  function kindLabel(node: ClientBoardNode): string {
    return node.kind === 'decision' ? 'Decision' : 'Work';
  }

  function containsControlCharacter(value: string): boolean {
    for (const character of value) {
      const codePoint = character.codePointAt(0);
      if (codePoint !== undefined && (codePoint <= 0x1f || codePoint === 0x7f)) {
        return true;
      }
    }
    return false;
  }

  let overview = $state<ClientBoardOverview | null>(null);
  let error = $state<ClientRefusal | null>(null);
  let loading = $state(false);
  let taskId = $state('');
  let batchTaskIds = $state('');
  let batchSource = $state<'github' | 'linear'>('github');
  let taskRefusal = $state<ClientRefusal | null>(null);
  let fetchingTask = $state(false);
  let selectedTask = $state<ClientImportedTask | null>(null);
  let prTitle = $state('');
  let prBody = $state('');
  let prBaseBranch = $state('main');
  let submittingPr = $state(false);
  let prRefusal = $state<ClientRefusal | null>(null);
  const MAX_BATCH_TASKS = 32;

  async function load() {
    loading = true;
    error = null;
    const result = await clientStore.queryBoard();
    if ('message' in result) {
      error = result;
      overview = null;
    } else {
      overview = result;
    }
    loading = false;
  }

  async function fetchTask() {
    const value = taskId.trim();
    taskRefusal = null;
    if (!value) {
      taskRefusal = { code: 'SCHEMA_INVALID', message: 'Enter a GitHub task such as octocat/hello#42.' };
      return;
    }
    if (value.length > 256 || containsControlCharacter(value)) {
      taskRefusal = { code: 'SCHEMA_INVALID', message: 'The task id is bounded and may not contain control characters.' };
      return;
    }
    fetchingTask = true;
    const refusal = await clientStore.dispatch({ type: 'fetchTask', source: 'github', taskId: value });
    if (refusal) {
      taskRefusal = refusal;
    } else {
      taskId = '';
      await load();
    }
    fetchingTask = false;
  }

  function parsedBatchTaskIds(): string[] {
    return [...new Set(batchTaskIds.split(/[\n,]+/).map((id) => id.trim()).filter(Boolean))];
  }

  async function fetchBatch() {
    const taskIds = parsedBatchTaskIds();
    taskRefusal = null;
    if (taskIds.length === 0) {
      taskRefusal = { code: 'SCHEMA_INVALID', message: 'Enter at least one task id to fetch.' };
      return;
    }
    if (taskIds.length > MAX_BATCH_TASKS) {
      taskRefusal = {
        code: 'SCHEMA_INVALID',
        message: `A batch may contain at most ${MAX_BATCH_TASKS} task ids.`
      };
      return;
    }
    if (taskIds.some((id) => id.length > 256 || containsControlCharacter(id))) {
      taskRefusal = {
        code: 'SCHEMA_INVALID',
        message: 'Each task id is bounded and may not contain control characters.'
      };
      return;
    }
    fetchingTask = true;
    const refusal = await clientStore.dispatch({
      type: 'fetchTasks',
      source: batchSource,
      taskIds
    });
    if (refusal) {
      taskRefusal = refusal;
    } else {
      batchTaskIds = '';
      await load();
    }
    fetchingTask = false;
  }

  function actsFor(task: ClientImportedTask, acts: ClientImportedAct[]): ClientImportedAct[] {
    return acts
      .filter((act) => act.itemBoardId === task.boardId)
      .sort((a, b) => a.actId.localeCompare(b.actId));
  }

  async function submitPr() {
    if (!selectedTask) return;
    prRefusal = null;
    const repository = clientStore.snapshot.repository;
    if (repository.freshness.type !== 'capturedAt' || !repository.branch || !repository.head) {
      prRefusal = { code: 'WORKSPACE_STALE_REVISION', message: 'Refresh repository state before submitting a pull request.' };
      return;
    }
    if (!prTitle.trim() || !prBody.trim() || !prBaseBranch.trim()) {
      prRefusal = { code: 'SCHEMA_INVALID', message: 'Pull request title, body, and base branch are required.' };
      return;
    }
    submittingPr = true;
    const refusal = await clientStore.dispatch({
      type: 'submitChange',
      source: selectedTask.integration,
      request: {
        remoteId: selectedTask.remoteId,
        expectedRevision: selectedTask.fetchedRevision,
        title: prTitle.trim(),
        body: prBody.trim(),
        headCommit: repository.head,
        headBranch: repository.branch,
        baseBranch: prBaseBranch.trim()
      }
    });
    if (refusal) {
      prRefusal = refusal;
    } else {
      selectedTask = null;
      prTitle = '';
      prBody = '';
      await load();
    }
    submittingPr = false;
  }

  onMount(() => {
    void load();
  });
</script>

<section class="flex min-h-0 h-full flex-col bg-background" data-testid="board-pane">
  <header class="flex shrink-0 items-center gap-2 border-b px-4 py-3">
    <div class="min-w-0 flex-1">
      <h2 class="font-medium">Board</h2>
      <p class="text-xs text-muted-foreground">
        What is decidable now, and why the rest is fogged
      </p>
    </div>
    <button
      type="button"
      class="text-xs text-muted-foreground underline-offset-2 hover:underline"
      onclick={() => void load()}
      disabled={loading || fetchingTask}
    >Refresh</button>
  </header>

  <div class="shrink-0 border-b px-4 py-3">
    <form class="flex items-center gap-2" onsubmit={(event) => { event.preventDefault(); void fetchTask(); }}>
      <label class="sr-only" for="github-task-id">GitHub task id</label>
      <input
        id="github-task-id"
        class="min-w-0 flex-1 rounded-md border bg-background px-2 py-1.5 text-xs"
        placeholder="octocat/hello#42"
        bind:value={taskId}
        maxlength="256"
        disabled={fetchingTask}
      />
      <button
        type="submit"
        class="rounded-md border px-2 py-1.5 text-xs hover:bg-muted disabled:opacity-50"
        disabled={fetchingTask}
      >{fetchingTask ? 'Fetching…' : 'Fetch GitHub task'}</button>
    </form>
    {#if taskRefusal}
      <p class="mt-2 text-xs text-destructive" role="alert">
        {#if taskRefusal.code}<span class="font-mono">{taskRefusal.code} · </span>{/if}{taskRefusal.message}
      </p>
    {/if}
    <div class="mt-3 border-t pt-3">
      <p class="mb-2 text-xs text-muted-foreground">
        Fetch several tasks in order. Successful items are recorded even if a later id refuses.
      </p>
      <form class="flex flex-col gap-2" onsubmit={(event) => { event.preventDefault(); void fetchBatch(); }}>
        <label class="sr-only" for="batch-task-source">Batch task source</label>
        <select
          id="batch-task-source"
          class="rounded-md border bg-background px-2 py-1.5 text-xs"
          aria-label="Batch task source"
          bind:value={batchSource}
          disabled={fetchingTask}
        >
          <option value="github">GitHub</option>
          <option value="linear">Linear</option>
        </select>
        <label class="sr-only" for="batch-task-ids">Batch task ids</label>
        <textarea
          id="batch-task-ids"
          class="min-h-16 rounded-md border bg-background px-2 py-1.5 text-xs"
          aria-label="Batch task ids"
          placeholder="One id per line or comma-separated"
          bind:value={batchTaskIds}
          disabled={fetchingTask}
        ></textarea>
        <button
          type="submit"
          class="self-start rounded-md border px-2 py-1.5 text-xs hover:bg-muted disabled:opacity-50"
          disabled={fetchingTask}
        >{fetchingTask ? 'Fetching…' : 'Fetch task batch'}</button>
      </form>
    </div>
  </div>

  <div class="min-h-0 flex-1 overflow-auto p-4">
    {#if loading}
      <p class="text-sm text-muted-foreground">Recomputing the frontier…</p>
    {:else if error}
      <div class="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm" role="alert">
        {#if error.code}<span class="font-mono text-xs">{error.code} · </span>{/if}{error.message}
      </div>
    {:else if overview}
      {#if overview.frontier.length === 0 && overview.fog.length === 0 && overview.settled.length === 0 && overview.importedTasks.length === 0}
        <Empty.Root>
          <Empty.Header>
            <Empty.Title>An empty board</Empty.Title>
            <Empty.Description>
              No decision tickets, plans, or imported work items are recorded in this
              workspace yet.
            </Empty.Description>
          </Empty.Header>
        </Empty.Root>
      {:else}
        {#if overview.importedTasks.length > 0}
          <div class="mb-5 flex flex-col gap-2">
            <h3 class="text-sm font-medium">Imported tasks</h3>
            {#each overview.importedTasks as task (task.boardId)}
              {@const taskActs = actsFor(task, overview.importedActs)}
              <div class="rounded-md border bg-muted/20 p-3">
                <div class="flex items-center gap-2">
                  <span class="min-w-0 flex-1 truncate text-sm" title={task.title}>{task.title}</span>
                  <Badge variant="outline">{task.integration}</Badge>
                  <Badge variant="outline">{task.state}</Badge>
                </div>
                <div class="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
                  <a class="truncate underline-offset-2 hover:underline" href={task.sourceUrl} target="_blank" rel="noreferrer">{task.remoteId}</a>
                  <span class="font-mono">{task.fetchedRevision}</span>
                  <button
                    type="button"
                    class="ml-auto shrink-0 rounded-md border px-2 py-1 hover:bg-muted"
                    onclick={() => {
                      selectedTask = task;
                      prTitle = task.title;
                      prBody = '';
                      prRefusal = null;
                    }}
                  >Prepare PR</button>
                </div>
                {#if taskActs.length > 0}
                  <div class="mt-2 flex flex-col gap-1 border-t pt-2 text-xs">
                    {#each taskActs as act (act.actId)}
                      <div class="flex items-center gap-2">
                        <Badge variant="outline">{act.kind}</Badge>
                        <span class="font-mono">{act.headBranch} → {act.baseBranch}</span>
                        {#if act.remoteUrl}
                          <a class="truncate underline-offset-2 hover:underline" href={act.remoteUrl} target="_blank" rel="noreferrer">{act.outcome}</a>
                        {:else}
                          <span class="text-amber-600">{act.outcome}</span>
                        {/if}
                        <span class="ml-auto font-mono text-muted-foreground">{act.expectedRevision}</span>
                      </div>
                    {/each}
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}

        {#if selectedTask}
          <div class="mb-5 rounded-md border border-primary/40 bg-primary/5 p-3">
            <div class="mb-2 flex items-center gap-2">
              <h3 class="min-w-0 flex-1 truncate text-sm font-medium">Submit PR for {selectedTask.remoteId}</h3>
              <button type="button" class="text-xs text-muted-foreground underline-offset-2 hover:underline" onclick={() => (selectedTask = null)}>Cancel</button>
            </div>
            <p class="mb-3 text-xs text-muted-foreground">Pinned to remote revision <span class="font-mono">{selectedTask.fetchedRevision}</span>. smed will recheck it before posting.</p>
            <div class="flex flex-col gap-2">
              <input class="rounded-md border bg-background px-2 py-1.5 text-xs" aria-label="Pull request title" bind:value={prTitle} maxlength="512" />
              <input class="rounded-md border bg-background px-2 py-1.5 text-xs" aria-label="Base branch" bind:value={prBaseBranch} maxlength="200" />
              <textarea class="min-h-20 rounded-md border bg-background px-2 py-1.5 text-xs" aria-label="Pull request body" bind:value={prBody} maxlength="32768"></textarea>
              <button type="button" class="self-start rounded-md border px-2 py-1.5 text-xs hover:bg-muted disabled:opacity-50" onclick={() => void submitPr()} disabled={submittingPr}>{submittingPr ? 'Submitting…' : 'Submit pull request'}</button>
            </div>
            {#if prRefusal}<p class="mt-2 text-xs text-destructive" role="alert">{#if prRefusal.code}<span class="font-mono">{prRefusal.code} · </span>{/if}{prRefusal.message}</p>{/if}
          </div>
        {/if}

        <div class="mb-4 flex flex-wrap gap-1.5 text-xs">
          <Badge variant="secondary">{overview.frontier.length} decidable</Badge>
          <Badge variant="outline">{overview.fog.length} fogged</Badge>
          <Badge variant="outline">{overview.settled.length} settled</Badge>
          {#if overview.cycles.length > 0}<Badge variant="outline">{overview.cycles.length} cycles</Badge>{/if}
        </div>

        {#if overview.frontier.length > 0}
          <h3 class="mb-2 text-sm font-medium">Decidable now</h3>
          <div class="mb-5 flex flex-col gap-2">
            {#each overview.frontier as node (node.id)}
              <div class="rounded-md border bg-background p-3">
                <div class="flex items-center gap-2">
                  <span class="min-w-0 flex-1 truncate text-sm" title={node.label}>{node.label}</span>
                  <Badge variant="outline">{kindLabel(node)}</Badge>
                  <Badge variant="outline">{provenanceLabel(node)}</Badge>
                </div>
              </div>
            {/each}
          </div>
        {/if}

        {#if overview.fog.length > 0}
          <h3 class="mb-2 text-sm font-medium">Fogged — waiting</h3>
          <div class="mb-5 flex flex-col gap-2">
            {#each overview.fog as fogged (fogged.node.id)}
              <div class="rounded-md border bg-muted/30 p-3">
                <div class="flex items-center gap-2">
                  <span class="min-w-0 flex-1 truncate text-sm" title={fogged.node.label}>
                    {fogged.node.label}
                  </span>
                  <Badge variant="outline">{kindLabel(fogged.node)}</Badge>
                  <Badge variant="outline">{provenanceLabel(fogged.node)}</Badge>
                </div>
                {#if fogged.waitsOn.length > 0}
                  <div class="mt-2 pl-3 text-xs text-muted-foreground">
                    <p class="mb-1">Why this is not decidable — waits on:</p>
                    <div class="flex flex-col gap-1 border-l-2 border-muted pl-2">
                      {#each fogged.waitsOn as blocker (blocker.id)}
                        <div class="flex items-center gap-1.5">
                          <span class="text-muted-foreground/60">·</span>
                          <span class="min-w-0 truncate" title={blocker.label}>{blocker.label}</span>
                          <span class="shrink-0">{kindLabel(blocker)} · {provenanceLabel(blocker)}</span>
                        </div>
                      {/each}
                    </div>
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}

        {#if overview.settled.length > 0}
          <h3 class="mb-2 text-sm font-medium">Settled</h3>
          <div class="mb-5 flex flex-col gap-2">
            {#each overview.settled as node (node.id)}
              <div class="rounded-md border bg-muted/20 p-3 opacity-70">
                <div class="flex items-center gap-2">
                  <span class="min-w-0 flex-1 truncate text-sm" title={node.label}>{node.label}</span>
                  <Badge variant="outline">{kindLabel(node)}</Badge>
                  <Badge variant="outline">{provenanceLabel(node)}</Badge>
                </div>
              </div>
            {/each}
          </div>
        {/if}

        {#if overview.cycles.length > 0}
          <h3 class="mb-2 text-sm font-medium">Blocking cycles</h3>
          <div class="flex flex-col gap-2">
            {#each overview.cycles as cycle, index (index)}
              <div class="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-xs">
                <p class="mb-1 font-medium">No member is decidable — members wait on each other:</p>
                <div class="flex flex-col gap-1 pl-2">
                  {#each cycle as member (member.id)}
                    <div class="flex items-center gap-1.5">
                      <span class="text-muted-foreground/60">·</span>
                      <span class="min-w-0 truncate" title={member.label}>{member.label}</span>
                    </div>
                  {/each}
                </div>
              </div>
            {/each}
          </div>
        {/if}
      {/if}
    {/if}
  </div>
</section>
