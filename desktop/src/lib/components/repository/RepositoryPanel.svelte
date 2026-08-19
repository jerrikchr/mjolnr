<script lang="ts">
  /**
   * The Repository panel (Phase D5 UI, layout per ADR 0009).
   *
   * ADR 0009 places this at the foot of the file explorer. The file explorer is
   * D7 and does not exist yet, so the panel is mounted in the sidebar footer
   * and moves when D7 lands — the rows and their wording are what this phase
   * owes, not the enclosing furniture.
   *
   * Two cells in the mockup assert states the runtime cannot honestly claim,
   * and ADR 0009 names both as traps this component must not reproduce:
   *
   * 1. `sync: synced` in the verified colour. ADR 0008: the counts describe the
   *    remote-tracking ref as smed last saw it, never the remote now. So the
   *    row always carries the as-of qualifier, and never a bare "synced". The
   *    qualifier is not conditional on a timestamp existing — a fresh clone
   *    writes its tracking ref with no reflog entry, so `remoteSyncAsOf` is
   *    routinely absent and the sentence has to stand without it.
   * 2. `branch main / head a1b2c3d / dirty 0` with no capture marker, as if it
   *    were the repository right now. `ClientRepositoryFreshness` has no
   *    `fresh` variant on purpose: nothing watches the filesystem, so the panel
   *    reports when git was asked and never that the answer still holds.
   *
   * Colour: no `--gov-verified`, and no colour carries meaning alone here. Every
   * state is legible as words.
   */
  import { clientStore, type ClientRefusal } from '$lib/runtime/client.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import { GitBranchIcon, RefreshIcon } from '@hugeicons/core-free-icons';
  import RepositoryControls from './RepositoryControls.svelte';
  import type {
    ClientRepositoryFreshness,
    ClientRepositoryState,
    ClientRepositorySyncState,
    ClientRepositoryHistory
  } from '$lib/runtime/contract';

  let repository = $derived<ClientRepositoryState>(clientStore.snapshot.repository);
  let freshness = $derived<ClientRepositoryFreshness>(repository.freshness);
  let sync = $derived<ClientRepositorySyncState>(repository.remoteSync);
  let stagedCount = $derived(repository.stagedFiles.length);
  let history = $state<ClientRepositoryHistory | null>(null);
  let historyRefusal = $state<ClientRefusal | null>(null);
  let historyBusy = $state(false);

  /**
   * How each trigger reads in a sentence beginning "read when…".
   *
   * The wire values are stable identifiers (`core::repository::RefreshTrigger`);
   * this is presentation only. An unrecognised trigger renders verbatim rather
   * than as "unknown" — a value smed sent is better evidence than a shrug.
   */
  const TRIGGER_PROSE: Record<string, string> = {
    projectOpened: 'the project was opened',
    repositoryCommand: 'a repository command completed',
    fileSave: 'a human editor save completed',
    toolWrite: 'a governed tool wrote to the workspace',
    requested: 'a refresh was requested'
  };

  function triggerProse(trigger: string): string {
    return TRIGGER_PROSE[trigger] ?? trigger;
  }

  /**
   * The sync row's own sentence.
   *
   * Every branch names the ref rather than the remote. `synced` is the one the
   * ADR calls a trap and it is spelled out longhand for exactly that reason;
   * shortening it back to one word is the regression the test guards.
   */
  function syncSentence(state: ClientRepositorySyncState): string {
    switch (state.type) {
      case 'unknown':
        return 'No upstream to compare against';
      case 'ahead':
        return `${state.count} commit${state.count === 1 ? '' : 's'} ahead of the ref last seen`;
      case 'behind':
        return `${state.count} commit${state.count === 1 ? '' : 's'} behind the ref last seen`;
      case 'diverged':
        return `${state.ahead} ahead, ${state.behind} behind the ref last seen`;
      case 'synced':
        return 'Level with the ref last seen';
    }
  }

  function refresh() {
    void clientStore.dispatch({ type: 'refreshRepository' });
  }

  async function loadHistory() {
    if (freshness.type !== 'capturedAt' || historyBusy) return;
    historyBusy = true;
    historyRefusal = null;
    const result = await clientStore.queryRepositoryHistory(20);
    historyBusy = false;
    if ('message' in result) {
      historyRefusal = result;
      return;
    }
    history = result;
  }
</script>

<section class="flex flex-col gap-2 text-xs" data-testid="repository-panel" aria-label="Repository">
  <header class="flex items-center justify-between gap-2">
    <div class="flex min-w-0 items-center gap-1.5 font-medium">
      <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} class="size-3.5 shrink-0" />
      <span>Repository</span>
    </div>
    <Button
      variant="ghost"
      size="icon-sm"
      aria-label="Re-read repository state"
      disabled={freshness.type === 'noProject'}
      onclick={refresh}
    >
      <HugeiconsIcon icon={RefreshIcon} strokeWidth={2} />
    </Button>
  </header>

  {#if freshness.type === 'noProject'}
    <p class="text-muted-foreground" data-testid="repository-no-project">
      No project is open, so git has not been asked anything.
    </p>
  {:else if freshness.type === 'unavailable'}
    <!--
      Distinct from "no project": the directory is there and git declined or
      could not answer. The reason code rides alongside the sentence, as it does
      at every other refusal in this client.
    -->
    <div class="flex flex-col gap-1" data-testid="repository-unavailable" role="status">
      <span class="font-mono">{freshness.code}</span>
      <span class="text-muted-foreground">{freshness.detail}</span>
    </div>
  {:else}
    <dl class="grid grid-cols-[auto_1fr] items-baseline gap-x-3 gap-y-1">
      <dt class="text-muted-foreground">branch</dt>
      <dd class="truncate font-mono" data-testid="repository-branch">
        {repository.branch ?? 'detached'}
      </dd>

      <dt class="text-muted-foreground">head</dt>
      <dd class="truncate font-mono" data-testid="repository-head">
        {repository.head ? repository.head.slice(0, 7) : 'unborn'}
      </dd>

      <dt class="text-muted-foreground">dirty</dt>
      <dd data-testid="repository-dirty">
        {repository.dirtyCount}{repository.dirtyCountTruncated ? '+' : ''}
        {#if repository.dirtyCountTruncated}
          <span class="text-muted-foreground">(counted to smed's bound, not to the end)</span>
        {/if}
      </dd>

      <dt class="text-muted-foreground">staged</dt>
      <dd data-testid="repository-staged">
        {stagedCount}{repository.pathsTruncated ? '+' : ''}
        {#if repository.pathsTruncated}
          <span class="text-muted-foreground">(path list truncated)</span>
        {/if}
      </dd>

      <dt class="text-muted-foreground">sync</dt>
      <!--
        Two lines, always. The count and the qualifier are one statement; a
        surface that drops the qualifier to save a line has broken ADR 0008's
        contract rather than tidied it.
      -->
      <dd class="flex flex-col gap-0.5" data-testid="repository-sync">
        <span>{syncSentence(sync)}</span>
        {#if sync.type === 'unknown'}
          <span class="text-muted-foreground">
            No upstream is configured, or git would not answer. This is not "smed did not look".
          </span>
        {:else}
          <span class="text-muted-foreground" data-testid="repository-sync-as-of">
            {#if repository.remoteSyncAsOf}
              As of {repository.remoteSyncAsOf}, the last time smed saw the remote. Whether it
              has moved since is not knowable without a fetch.
            {:else}
              As of the last time smed saw the remote — no timestamp was recorded for it.
              Whether the remote has moved since is not knowable without a fetch.
            {/if}
          </span>
        {/if}
      </dd>

      {#if repository.unmergedFiles.length > 0}
        <dt class="text-muted-foreground">conflicts</dt>
        <dd data-testid="repository-unmerged">
          {`${repository.unmergedFiles.length} path${
            repository.unmergedFiles.length === 1 ? '' : 's'
          } unmerged`}
          <span class="text-muted-foreground">— resolve before staging</span>
        </dd>
      {/if}
    </dl>

    <!--
      The capture marker. Rendered for every read, including a clean one: the
      mockup's version of this panel omitted it, which is what turned a reading
      into a claim.
    -->
    <p class="text-muted-foreground" data-testid="repository-freshness">
      Read when {triggerProse(freshness.trigger)} (capture #{freshness.sequence}). Nothing watches
      the filesystem, so this is what git said then, not what it would say now.
    </p>

    <div class="flex flex-col gap-1.5 border-t pt-2" data-testid="repository-history">
      <div class="flex items-center justify-between gap-2">
        <span class="font-medium">Recent history</span>
        <Button variant="ghost" size="sm" disabled={historyBusy} onclick={loadHistory}>
          {historyBusy ? 'Reading…' : history ? 'Refresh history' : 'Show history'}
        </Button>
      </div>
      {#if historyRefusal}
        <p class="text-destructive" role="alert">{historyRefusal.message}</p>
      {:else if history}
        {#if history.entries.length === 0}
          <p class="text-muted-foreground">No commits are recorded yet.</p>
        {:else}
          <ol class="flex flex-col gap-1.5" aria-label="Recent commits">
            {#each history.entries as entry}
              <li class="min-w-0 rounded-md border p-2">
                <div class="flex min-w-0 items-baseline justify-between gap-2">
                  <span class="truncate font-medium" title={entry.subject}>{entry.subject}</span>
                  <span class="shrink-0 font-mono text-[0.65rem] text-muted-foreground">
                    {entry.revision.slice(0, 7)}
                  </span>
                </div>
                <div class="truncate text-[0.65rem] text-muted-foreground">
                  {entry.author} · {entry.authoredAt}
                </div>
              </li>
            {/each}
          </ol>
          {#if history.hasMore}
            <p class="text-[0.65rem] text-muted-foreground">
              Showing the newest {history.limit}; older commits remain in git.
            </p>
          {/if}
        {/if}
      {/if}
    </div>
  {/if}

  <Badge variant="outline" class="w-fit">{repository.trust}</Badge>

  <RepositoryControls />
</section>
