<script lang="ts">
  import { onMount } from 'svelte';
  import { clientStore, type ClientRefusal } from '$lib/runtime/client.svelte';
  import type { ClientGraphDirection, ClientGraphNode, ClientGraphPage } from '$lib/runtime/contract';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import * as Empty from '$lib/components/ui/empty';
  import { Input } from '$lib/components/ui/input';

  let {
    onopen
  }: {
    onopen?: (path: string) => void;
  } = $props();

  let page = $state<ClientGraphPage | null>(null);
  let error = $state<ClientRefusal | null>(null);
  let loading = $state(false);
  let direction = $state<ClientGraphDirection>('both');
  let focusPath = $state<string | null>(null);
  let search = $state('');

  async function load(path: string | null = focusPath) {
    loading = true;
    error = null;
    const trimmed = search.trim();
    const result = await clientStore.queryGraph({
      path,
      depth: 2,
      direction,
      search: trimmed ? trimmed : null
    });
    if ('message' in result) {
      error = result;
      page = null;
    } else {
      page = result;
    }
    loading = false;
  }

  function focus(node: ClientGraphNode) {
    focusPath = node.path;
    void load(node.path);
  }

  function reset() {
    focusPath = null;
    void load(null);
  }

  onMount(() => {
    void load(null);
  });
</script>

<section class="flex min-h-0 h-full flex-col bg-background" data-testid="graph-pane">
  <header class="flex shrink-0 items-center gap-2 border-b px-4 py-3">
    <div class="min-w-0 flex-1">
      <h2 class="font-medium">Code graph</h2>
      <p class="text-xs text-muted-foreground">
        Deterministic source relationships · parsed edges only
      </p>
    </div>
    <Input
      class="h-8 w-44"
      placeholder="Search path or symbol"
      aria-label="Graph search"
      bind:value={search}
      onkeydown={(event: KeyboardEvent) => {
        if (event.key === 'Enter') void load(focusPath);
        if (event.key === 'Escape') { search = ''; void load(focusPath); }
      }}
    />
    <Button variant="ghost" size="sm" onclick={() => void load(focusPath)}>Search</Button>
    <select
      class="h-8 rounded-md border bg-background px-2 text-xs"
      aria-label="Graph direction"
      bind:value={direction}
      onchange={() => void load(focusPath)}
    >
      <option value="both">imports + importers</option>
      <option value="imports">imports</option>
      <option value="importers">importers</option>
    </select>
    <Button variant="ghost" size="sm" onclick={reset} disabled={!focusPath}>Overview</Button>
  </header>

  <div class="min-h-0 flex-1 overflow-auto p-4">
    {#if loading}
      <p class="text-sm text-muted-foreground">Building deterministic graph…</p>
    {:else if error}
      <div class="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm" role="alert">
        {#if error.code}<span class="font-mono text-xs">{error.code} · </span>{/if}{error.message}
      </div>
    {:else if page}
      <div class="mb-4 flex flex-wrap gap-1.5 text-xs">
        <Badge variant="secondary">{page.nodes.length} shown</Badge>
        <Badge variant="outline">{page.edges.length} edges</Badge>
        <Badge variant="outline">{page.summary.filesScanned} source files</Badge>
        {#if page.truncated}<Badge variant="outline">bounded</Badge>{/if}
        {#if page.summary.nonParsedEdges > 0}<Badge variant="outline">{page.summary.nonParsedEdges} non-parsed edges</Badge>{/if}
      </div>
      <div class="mb-2 flex flex-wrap gap-1.5 text-xs text-muted-foreground">
        <span>{page.summary.externalImports} external</span>
        <span>{page.summary.unresolvedImports} unresolved</span>
        {#if page.summary.filesSkipped > 0}<span>{page.summary.filesSkipped} skipped</span>{/if}
        {#if page.summary.filesTooLarge > 0}<span>{page.summary.filesTooLarge} too large</span>{/if}
      </div>
      {#if focusPath}
        <p class="mb-3 truncate font-mono text-xs text-muted-foreground" title={focusPath}>
          Focus: {focusPath}
        </p>
      {/if}
      {#if page.nodes.length === 0}
        <Empty.Root>
          <Empty.Header>
            <Empty.Title>No graph nodes</Empty.Title>
            <Empty.Description
              >{search.trim()
                ? `No nodes match "${search.trim()}".`
                : 'No supported source files were observed in this workspace.'}</Empty.Description
            >
          </Empty.Header>
        </Empty.Root>
      {:else}
        <div class="flex flex-col gap-2">
          {#each page.nodes as node (node.path)}
            <Card.Root>
              <Card.Header class="p-3">
                <div class="flex items-center gap-2">
                  <button
                    type="button"
                    class="min-w-0 truncate text-left font-mono text-sm text-accent-bright hover:underline"
                    title={`Focus ${node.path}`}
                    onclick={() => focus(node)}
                  >{node.path}</button>
                  <Badge variant="outline" class="ml-auto shrink-0">{node.language}</Badge>
                </div>
              </Card.Header>
              <Card.Content class="p-3 pt-0">
                <div class="flex flex-wrap gap-2 text-xs text-muted-foreground">
                  <span>{node.imports.length} imports</span>
                  <span>{node.importers.length} importers</span>
                  <span>{node.symbols.length} symbols</span>
                  {#if node.distance !== null && node.distance !== undefined}<span>distance {node.distance}</span>{/if}
                </div>
                <div class="mt-2 flex flex-wrap gap-1.5">
                  {#each node.symbols.slice(0, 8) as symbol}
                    <Badge variant="secondary" class="font-mono text-xs">{symbol.kind} {symbol.name}:{symbol.line}</Badge>
                  {/each}
                </div>
                {#if onopen}
                  <Button variant="ghost" size="sm" class="mt-2" onclick={() => onopen?.(node.path)}>Open in editor</Button>
                {/if}
              </Card.Content>
            </Card.Root>
          {/each}
        </div>
        {#if page.edges.length > 0}
          <div class="mt-4 rounded-md border p-3">
            <p class="mb-2 text-xs font-medium">Edges in view</p>
            <div class="flex flex-col gap-1 font-mono text-xs">
              {#each page.edges.slice(0, 24) as edge}
                <span>{edge.from} → {edge.to} <span class="text-muted-foreground">({edge.provenance})</span></span>
              {/each}
              {#if page.edges.length > 24}<span class="text-muted-foreground">…{page.edges.length - 24} more</span>{/if}
            </div>
          </div>
        {/if}
      {/if}
    {/if}
  </div>
</section>
