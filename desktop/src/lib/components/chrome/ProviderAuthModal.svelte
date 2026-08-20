<!--
  ProviderAuthModal: Direct modal for inspecting, connecting, and verifying model providers
  (Anthropic, OpenAI, Gemini, OpenRouter, Ollama) without requiring full page navigation.
-->
<script lang="ts">
  import { goto } from '$app/navigation';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import {
    SparklesIcon,
    RefreshIcon,
    ArrowRight01Icon,
    CheckmarkCircle02Icon,
    Key01Icon,
    CpuIcon
  } from '@hugeicons/core-free-icons';
  import { clientStore } from '$lib/runtime/client.svelte';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import StatusOrb from './StatusOrb.svelte';

  let {
    open = $bindable(false)
  }: {
    open?: boolean;
  } = $props();

  let snap = $derived(clientStore.snapshot);
  let accounts = $derived(snap.accounts);
  let models = $derived(snap.models);
  let refreshing = $state(false);

  const providerList = [
    {
      id: 'anthropic',
      name: 'Anthropic',
      description: 'Claude 3.7 Sonnet, Claude 3.5 Haiku',
      envVar: 'ANTHROPIC_API_KEY',
      docsUrl: 'https://console.anthropic.com/settings/keys'
    },
    {
      id: 'openai',
      name: 'OpenAI',
      description: 'GPT-4o, o3-mini, o1',
      envVar: 'OPENAI_API_KEY',
      docsUrl: 'https://platform.openai.com/api-keys'
    },
    {
      id: 'gemini',
      name: 'Google Gemini',
      description: 'Gemini 2.5 Pro, Flash',
      envVar: 'GEMINI_API_KEY',
      docsUrl: 'https://aistudio.google.com/app/apikey'
    },
    {
      id: 'openrouter',
      name: 'OpenRouter',
      description: 'Universal model aggregator',
      envVar: 'OPENROUTER_API_KEY',
      docsUrl: 'https://openrouter.ai/keys'
    },
    {
      id: 'ollama',
      name: 'Ollama (Local)',
      description: 'Local models running on localhost:11434',
      envVar: 'OLLAMA_HOST (optional)',
      docsUrl: 'https://ollama.com'
    }
  ];

  function getAccountState(providerId: string) {
    const found = accounts.find((a) => a.provider.toLowerCase() === providerId.toLowerCase());
    return found?.state ?? (models.some((m) => m.provider.toLowerCase() === providerId.toLowerCase()) ? 'connected' : 'disconnected');
  }

  function getProviderModels(providerId: string) {
    return models.filter((m) => m.provider.toLowerCase() === providerId.toLowerCase());
  }

  async function handleRefresh() {
    refreshing = true;
    try {
      await clientStore.dispatch({ type: 'requestSnapshot' });
      await new Promise((r) => setTimeout(r, 400));
    } finally {
      refreshing = false;
    }
  }

  function openGuidedSetup() {
    open = false;
    goto('/onboarding');
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-2xl max-h-[85vh] overflow-y-auto bg-card border-border/80 shadow-2xl p-6">
    <Dialog.Header class="pb-3 border-b border-border/50">
      <div class="flex items-center justify-between">
        <Dialog.Title class="flex items-center gap-2.5 text-lg font-bold text-foreground">
          <HugeiconsIcon icon={SparklesIcon} strokeWidth={2} class="size-5 text-primary" />
          <span>Connect Model Providers</span>
        </Dialog.Title>
        <Badge variant={models.length > 0 ? 'default' : 'secondary'} class="font-mono text-xs">
          {models.length} model{models.length === 1 ? '' : 's'} available
        </Badge>
      </div>
      <Dialog.Description class="text-xs text-muted-foreground mt-1">
        mjolnr connects directly to model provider APIs. Set environment variables or store keys in your environment.
      </Dialog.Description>
    </Dialog.Header>

    <div class="space-y-3 py-4">
      {#each providerList as provider (provider.id)}
        {@const state = getAccountState(provider.id)}
        {@const provModels = getProviderModels(provider.id)}
        <div class="flex flex-col gap-2 rounded-xl border border-border/70 bg-muted/20 p-3.5 transition-colors hover:border-border">
          <div class="flex items-center justify-between gap-3">
            <div class="flex items-center gap-2.5">
              <StatusOrb state={state === 'connected' ? 'verified' : state === 'needsReauth' ? 'attention' : 'idle'} size={7} />
              <div>
                <h4 class="text-sm font-semibold text-foreground">{provider.name}</h4>
                <p class="text-xs text-muted-foreground">{provider.description}</p>
              </div>
            </div>
            <div>
              {#if state === 'connected'}
                <Badge variant="outline" class="gap-1 border-primary/40 bg-primary/10 text-primary text-[11px]">
                  <HugeiconsIcon icon={CheckmarkCircle02Icon} strokeWidth={2} class="size-3" />
                  Connected ({provModels.length})
                </Badge>
              {:else if state === 'needsReauth'}
                <Badge variant="destructive" class="text-[11px]">Needs Reauth</Badge>
              {:else}
                <Badge variant="secondary" class="text-[11px] text-muted-foreground">Not Connected</Badge>
              {/if}
            </div>
          </div>

          <div class="flex items-center justify-between gap-2 pt-2 border-t border-border/40 text-xs">
            <div class="flex items-center gap-1.5 font-mono text-[11px] text-muted-foreground">
              <HugeiconsIcon icon={Key01Icon} class="size-3 text-muted-foreground" />
              <span>Env: <code class="bg-background/80 px-1.5 py-0.5 rounded border border-border/60 text-foreground">{provider.envVar}</code></span>
            </div>
            <a
              href={provider.docsUrl}
              target="_blank"
              rel="noopener noreferrer"
              class="text-primary hover:underline text-[11px] font-medium"
            >
              Get API Key ↗
            </a>
          </div>
        </div>
      {/each}
    </div>

    <!-- Quick Environment Helper Info -->
    <div class="rounded-lg border border-border/60 bg-background/50 p-3 text-xs text-muted-foreground flex flex-col gap-1.5">
      <div class="flex items-center gap-1.5 font-medium text-foreground">
        <HugeiconsIcon icon={CpuIcon} class="size-3.5 text-primary" />
        <span>How to authenticate:</span>
      </div>
      <p class="text-[11px] leading-relaxed">
        Export your API keys in your shell profile (<code class="font-mono text-foreground">export ANTHROPIC_API_KEY=...</code>) or launch mjolnr from an environment where keys are active.
      </p>
    </div>

    <Dialog.Footer class="flex flex-wrap items-center justify-between gap-3 pt-3 border-t border-border/50 mt-2">
      <Button variant="ghost" size="sm" class="text-xs text-muted-foreground hover:text-foreground" onclick={openGuidedSetup}>
        <span>Open full guided setup</span>
        <HugeiconsIcon icon={ArrowRight01Icon} strokeWidth={2} class="size-3.5 ml-1" />
      </Button>
      <div class="flex items-center gap-2">
        <Button variant="outline" size="sm" class="gap-1.5 text-xs font-semibold" disabled={refreshing} onclick={handleRefresh}>
          <HugeiconsIcon icon={RefreshIcon} strokeWidth={2} class={refreshing ? 'size-3.5 animate-spin' : 'size-3.5'} />
          <span>{refreshing ? 'Checking...' : 'Check & Refresh'}</span>
        </Button>
        <Button size="sm" class="text-xs font-semibold" onclick={() => (open = false)}>
          Done
        </Button>
      </div>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
