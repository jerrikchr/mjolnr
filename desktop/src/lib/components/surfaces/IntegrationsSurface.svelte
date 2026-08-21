<script lang="ts">
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import { Key01Icon, PlugSocketIcon, CloudIcon, PuzzleIcon } from '@hugeicons/core-free-icons';
  import { Button } from '$lib/components/ui/button';
  import { Badge } from '$lib/components/ui/badge';
  import StatusOrb from '$lib/components/chrome/StatusOrb.svelte';
  import { clientStore } from '$lib/runtime/client.svelte';
  import { onMount } from 'svelte';

  let { onopenconnections }: { onopenconnections?: () => void } = $props();
  let snap = $derived(clientStore.snapshot);
  let julesConnected = $state(false);
  onMount(() => {
    void clientStore.authJulesStatus().then((connected) => (julesConnected = connected));
  });
</script>

<div class="mx-auto flex w-full max-w-5xl flex-col gap-5 p-6">
  <div>
    <p class="text-xs font-semibold uppercase tracking-[0.18em] text-primary">Workspace connections</p>
    <h1 class="mt-1 text-2xl font-semibold tracking-tight">Integrations, plugins &amp; MCP</h1>
    <p class="mt-1 max-w-2xl text-sm text-muted-foreground">
      Connect external systems here. Model providers, cloud agents, and tools keep separate trust and execution boundaries.
    </p>
  </div>

  <div class="grid gap-3 md:grid-cols-3">
    <section class="rounded-xl border border-border/70 bg-card p-4">
      <div class="flex items-center gap-2"><HugeiconsIcon icon={Key01Icon} class="size-4 text-primary" /><h2 class="font-medium">Model providers</h2></div>
      <p class="mt-2 text-xs text-muted-foreground">LLM accounts used by local sessions and routing.</p>
      <div class="mt-3 flex items-center gap-2"><StatusOrb state={snap.accounts.some((a) => a.state === 'connected') ? 'verified' : 'idle'} size={6} /><span class="text-xs">{snap.accounts.filter((a) => a.state === 'connected').length} connected</span></div>
    </section>
    <section class="rounded-xl border border-border/70 bg-card p-4">
      <div class="flex items-center gap-2"><HugeiconsIcon icon={CloudIcon} class="size-4 text-primary" /><h2 class="font-medium">Cloud agents</h2></div>
      <p class="mt-2 text-xs text-muted-foreground">Asynchronous remote execution remains externally unverified until local checks pass.</p>
      <div class="mt-3 flex items-center gap-2"><StatusOrb state={julesConnected ? 'verified' : 'idle'} size={6} /><span class="text-xs">Jules {julesConnected ? 'connected' : 'not connected'}</span></div>
    </section>
    <section class="rounded-xl border border-border/70 bg-card p-4">
      <div class="flex items-center gap-2"><HugeiconsIcon icon={PuzzleIcon} class="size-4 text-primary" /><h2 class="font-medium">Plugins &amp; MCP</h2></div>
      <p class="mt-2 text-xs text-muted-foreground">Governed tools and community extensions will appear here when configured.</p>
      <Badge variant="secondary" class="mt-3">No connections configured</Badge>
    </section>
  </div>

  <div class="flex items-center justify-between rounded-xl border border-dashed border-border/70 bg-muted/20 p-4">
    <div class="flex items-center gap-3"><HugeiconsIcon icon={PlugSocketIcon} class="size-5 text-primary" /><div><p class="text-sm font-medium">Manage connections</p><p class="text-xs text-muted-foreground">Open the governed connection flow for providers and Jules.</p></div></div>
    <Button variant="outline" size="sm" onclick={onopenconnections}>Open connection setup</Button>
  </div>
</div>
