<!--
  ProviderAuthModal: Connections surface — providers and cloud integrations.
  Searchable, grouped provider cards with inline auth forms.
-->
<script lang="ts">
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import {
    SparklesIcon,
    RefreshIcon,
    CheckmarkCircle02Icon,
    Key01Icon,
    Search01Icon,
    ArrowRight02Icon,
    PlugSocketIcon
  } from '@hugeicons/core-free-icons';
  import { clientStore } from '$lib/runtime/client.svelte';
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import StatusOrb from './StatusOrb.svelte';

  let {
    open = $bindable(false)
  }: {
    open?: boolean;
  } = $props();

  let snap = $derived(clientStore.snapshot);
  let refreshing = $state(false);
  let query = $state('');
  let connectingProvider = $state<string | null>(null);
  let connectingError = $state<string | null>(null);
  let lmStudioAddress = $state('http://localhost:1234');
  let lmStudioToken = $state('');
  let apiKeyInput = $state('');
  let julesConnected = $state(false);

  onMount(() => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    void listen<string>('mjolnr-oauth-authorize', (event) => {
      window.open(event.payload, '_blank', 'noopener,noreferrer');
    });
  });

  type Card = {
    provider: string;
    state: string;
    detail?: string;
    models: string[];
    kind: 'connected' | 'connecting' | 'needsAuth' | 'disconnected' | 'unavailable' | 'cloudAgent';
  };

  function cardKind(state: string): Card['kind'] {
    switch (state) {
      case 'connected': return 'connected';
      case 'discovering': return 'connecting';
      case 'needsReauth': return 'needsAuth';
      case 'unavailable': return 'unavailable';
      default: return 'disconnected';
    }
  }

  function docsUrlFor(provider: string): string {
    switch (provider) {
      case 'anthropic': return 'https://console.anthropic.com/settings/keys';
      case 'openai': return 'https://platform.openai.com/api-keys';
      case 'openai-codex': return 'https://chatgpt.com/codex';
      case 'gemini': return 'https://aistudio.google.com/app/apikey';
      case 'gemini-cli': return 'https://github.com/google-gemini/gemini-cli';
      case 'antigravity': return 'https://github.com/google-gemini/gemini-cli';
      case 'openrouter': return 'https://openrouter.ai/keys';
      case 'ollama': return 'https://ollama.com';
      case 'lm-studio': return 'https://lmstudio.ai/docs/app/api';
      default: return '';
    }
  }

  function connectedCountFor(provider: string, models: ClientModel[]): number {
    return models.filter((m) => m.provider.toLowerCase() === provider.toLowerCase()).length;
  }

  type ClientModel = { provider: string; model: string; displayName: string };

  let cards: Card[] = $derived(
    [
      ...snap.accounts.map((account) => ({
      provider: account.provider,
      state: account.state,
      detail: account.detail,
      models: snap.models
        .filter((m) => m.provider.toLowerCase() === account.provider.toLowerCase())
        .map((m) => m.model),
      kind: cardKind(account.state)
      })),
      ...(snap.accounts.some((account) => account.provider === 'jules')
        ? []
        : [{ provider: 'jules', state: julesConnected ? 'connected' : 'disconnected', models: [], kind: 'cloudAgent' as const }])
    ]
  );

  let filtered = $derived(
    (() => {
      const needle = query.trim().toLowerCase();
      if (!needle) return cards;
      return cards.filter((card) =>
        `${card.provider} ${card.state} ${card.detail ?? ''}`
          .toLowerCase()
          .includes(needle)
      );
    })()
  );

  let grouped = $derived((() => {
    const order: Record<Card['kind'], number> = { connected: 0, cloudAgent: 1, connecting: 2, needsAuth: 3, unavailable: 4, disconnected: 5 };
    const groups = new Map<Card['kind'], Card[]>();
    for (const card of filtered) {
      const bucket = groups.get(card.kind) ?? [];
      bucket.push(card);
      groups.set(card.kind, bucket);
    }
    return Array.from(groups.entries()).sort((a, b) => order[a[0]] - order[b[0]]);
  })());

  let connectedTotal = $derived(cards.filter((c) => c.kind === 'connected').length);

  function kindLabel(kind: Card['kind']): string {
    switch (kind) {
      case 'cloudAgent': return 'Cloud integrations';
      case 'connected': return 'Connected';
      case 'connecting': return 'Discovering';
      case 'needsAuth': return 'Needs Reauth';
      case 'unavailable': return 'Unavailable';
      default: return 'Not Connected';
    }
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

  function startConnect(provider: string) {
    connectingError = null;
    connectingProvider = provider;
    if (provider === 'lm-studio') {
      lmStudioAddress = 'http://localhost:1234';
      lmStudioToken = '';
    } else {
      apiKeyInput = '';
    }
  }

  function isGoogleOAuth(provider: string): boolean {
    return provider === 'gemini-cli' || provider === 'antigravity';
  }

  async function doGoogleOAuth(provider: string) {
    connectingError = null;
    const result = await clientStore.authGoogleOAuth(provider);
    if (result === true) {
      connectingProvider = null;
      await handleRefresh();
    } else {
      connectingError = result.error;
    }
  }

  function cancelConnect() {
    connectingProvider = null;
    connectingError = null;
  }

  async function doLmStudioConnect() {
    connectingError = null;
    const result = await clientStore.authLmStudioLogin(lmStudioAddress, lmStudioToken);
    if ('endpoint' in result) {
      connectingProvider = null;
      await handleRefresh();
    } else {
      connectingError = result.error;
    }
  }

  async function doApiKeyConnect(provider: string) {
    connectingError = null;
    const result = provider === 'jules'
      ? await clientStore.authJulesLogin(apiKeyInput)
      : await clientStore.authApiKeyLogin(provider, apiKeyInput);
    if (result === true) {
      if (provider === 'jules') julesConnected = true;
      connectingProvider = null;
      await handleRefresh();
    } else {
      connectingError = result.error;
    }
  }

  async function doLogout(provider: string) {
    await clientStore.authLogout(provider);
    if (provider === 'jules') julesConnected = false;
    await handleRefresh();
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="w-[min(96vw,980px)] max-w-[980px] sm:max-w-[980px] max-h-[88vh] overflow-hidden bg-card border-border/80 shadow-2xl p-0 gap-0 flex flex-col">
      <Dialog.Header class="px-6 pt-5 pb-3 border-b border-border/50 shrink-0">
      <div class="flex items-center justify-between gap-3">
        <Dialog.Title class="flex items-center gap-2.5 text-lg font-bold text-foreground">
          <HugeiconsIcon icon={SparklesIcon} strokeWidth={2} class="size-5 text-primary" />
          <span>Connections</span>
        </Dialog.Title>
        <Badge variant={connectedTotal > 0 ? 'default' : 'secondary'} class="font-mono text-xs shrink-0">
          {connectedTotal} connected · {cards.length} available
        </Badge>
      </div>
      <div class="relative mt-3">
        <HugeiconsIcon icon={Search01Icon} strokeWidth={2} class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          placeholder="Filter connections · e.g. Jules, lm-studio, openai"
          class="h-8 pl-8 text-sm"
          bind:value={query}
        />
      </div>
    </Dialog.Header>

    <div class="min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:px-6">
      {#if filtered.length === 0}
        <div class="rounded-lg border border-dashed border-border/60 bg-muted/20 px-4 py-8 text-center text-sm text-muted-foreground">
          No provider matches that filter.
        </div>
      {:else}
        <div class="space-y-5">
          {#each grouped as [kind, group] (kind)}
            <div class="space-y-2">
              <div class="flex items-center gap-2">
                <span
                  class="rounded-full px-2 py-0.5 text-[11px] font-semibold tracking-wide
                    {kind === 'connected' ? 'bg-gov-verified-bg text-gov-verified border border-gov-verified-border' : ''}
                    {kind === 'connecting' ? 'bg-gov-proposal-bg text-gov-proposal border border-gov-proposal-border' : ''}
                    {kind === 'cloudAgent' ? 'bg-gov-proposal-bg text-gov-proposal border border-gov-proposal-border' : ''}
                    {kind === 'needsAuth' ? 'bg-gov-approval-bg text-gov-approval border border-gov-approval-border' : ''}
                    {kind === 'unavailable' ? 'bg-gov-refusal-bg text-gov-refusal border border-gov-refusal-border' : ''}
                    {kind === 'disconnected' ? 'bg-muted text-muted-foreground border border-border/60' : ''}"
                >
                  {kindLabel(kind)} · {group.length}
                </span>
              </div>

              <div class="grid gap-2.5">
                {#each group as provider (provider.provider)}
                  {@const count = connectedCountFor(provider.provider, snap.models as unknown as ClientModel[])}
                  <div
                    class="flex flex-col gap-2 rounded-xl border bg-card p-3.5 transition-colors
                      {provider.kind === 'connected' ? 'border-gov-verified-border bg-gov-verified-bg/40' : 'border-border/70 hover:border-border'}"
                    data-testid="provider-card-{provider.provider}"
                  >
                    <div class="flex items-start justify-between gap-3">
                      <div class="flex items-start gap-2.5 min-w-0">
                        <StatusOrb
                          state={provider.kind === 'connected' ? 'verified' : provider.kind === 'needsAuth' ? 'attention' : provider.kind === 'connecting' ? 'active' : 'idle'}
                          size={7}
                        />
                        <div class="min-w-0">
                          <h4 class="truncate text-sm font-semibold text-foreground">{provider.provider}</h4>
                          {#if provider.detail}
                            <p class="mt-1 truncate text-[11px] text-muted-foreground">{provider.detail}</p>
                          {/if}
                        </div>
                      </div>
                      <div class="shrink-0 flex flex-col items-end gap-1.5">
                        {#if provider.kind === 'connected'}
                          <Badge variant="outline" class="gap-1 border-gov-verified-border bg-gov-verified-bg text-gov-verified text-[11px]">
                            <HugeiconsIcon icon={CheckmarkCircle02Icon} strokeWidth={2} class="size-3" />
                            Connected{count ? ` · ${count} model${count === 1 ? '' : 's'}` : ''}
                          </Badge>
                          <button type="button" class="text-[10px] text-muted-foreground hover:text-destructive transition-colors" onclick={() => doLogout(provider.provider)}>
                            Disconnect
                          </button>
                        {:else if provider.kind === 'connecting'}
                          <Badge variant="secondary" class="text-[11px]">Discovering…</Badge>
                        {:else if provider.kind === 'needsAuth'}
                          <Badge variant="destructive" class="text-[11px]">Needs Reauth</Badge>
                          <Button variant="outline" size="sm" class="h-6 text-[10px] gap-1 px-2" onclick={() => startConnect(provider.provider)}>
                            <HugeiconsIcon icon={PlugSocketIcon} strokeWidth={2} class="size-3" />
                            Reconnect
                          </Button>
                        {:else if provider.kind === 'unavailable'}
                          <Badge variant="destructive" class="text-[11px]">Unavailable</Badge>
                        {:else if isGoogleOAuth(provider.provider)}
                          <p class="text-[11px] font-medium text-foreground">Google account</p>
                          <p class="text-[11px] text-muted-foreground">A browser window will open to complete OAuth securely.</p>
                        {:else}
                          <Button variant="outline" size="sm" class="h-6 text-[10px] gap-1 px-2" onclick={() => startConnect(provider.provider)}>
                            <HugeiconsIcon icon={PlugSocketIcon} strokeWidth={2} class="size-3" />
                            Connect
                          </Button>
                        {/if}
                      </div>
                    </div>

                    {#if provider.models.length > 0}
                      <div class="flex flex-wrap gap-1 pt-1">
                        {#each provider.models.slice(0, 6) as model (model)}
                          <span class="rounded-full bg-muted px-2 py-0.5 font-mono text-[10px] text-muted-foreground border border-border/60">{model}</span>
                        {/each}
                        {#if provider.models.length > 6}
                          <span class="text-[11px] text-muted-foreground">+{provider.models.length - 6} more</span>
                        {/if}
                      </div>
                    {/if}

                    <!-- Inline auth form -->
                    {#if connectingProvider === provider.provider}
                      <div class="rounded-lg border border-primary/30 bg-muted/20 px-3 py-3 space-y-2.5">
                        {#if provider.provider === 'lm-studio'}
                          <p class="text-[11px] font-medium text-foreground">LM Studio Server</p>
                          <div class="space-y-2">
                            <Input placeholder="Server address" class="h-7 text-xs" bind:value={lmStudioAddress} />
                            <Input placeholder="API token (optional — blank = keyless)" class="h-7 text-xs" bind:value={lmStudioToken} />
                          </div>
                        {:else}
                          <p class="text-[11px] font-medium text-foreground">API Key</p>
                          <Input placeholder="Paste your API key" class="h-7 text-xs" bind:value={apiKeyInput} />
                        {/if}
                        {#if connectingError}
                          <p class="text-[11px] text-destructive">{connectingError}</p>
                        {/if}
                        <div class="flex items-center gap-2 justify-end">
                          <Button variant="ghost" size="sm" class="h-6 text-[10px]" onclick={cancelConnect}>
                            Cancel
                          </Button>
                          <Button
                            size="sm"
                            class="h-6 text-[10px] gap-1"
                            onclick={provider.provider === 'lm-studio' ? doLmStudioConnect : isGoogleOAuth(provider.provider) ? () => doGoogleOAuth(provider.provider) : () => doApiKeyConnect(provider.provider)}
                          >
                            <HugeiconsIcon icon={ArrowRight02Icon} strokeWidth={2} class="size-3" />
                            {provider.provider === 'lm-studio' ? 'Connect LM Studio' : isGoogleOAuth(provider.provider) ? 'Continue with Google' : 'Save Key'}
                          </Button>
                        </div>
                      </div>
                    {/if}

                    <div class="flex flex-wrap items-center justify-between gap-2 border-t border-border/40 pt-2 text-xs">
                      <div class="flex min-w-0 items-center gap-1.5 font-mono text-[11px] text-muted-foreground">
                        <HugeiconsIcon icon={Key01Icon} class="size-3 shrink-0 text-muted-foreground" />
                        <span class="truncate">Env: <code class="rounded border border-border/60 bg-background/80 px-1.5 py-0.5 text-foreground">{provider.provider === 'lm-studio' ? 'LM_API_TOKEN' : `${provider.provider.toUpperCase().replace(/-/g, '_')}_API_KEY`}</code></span>
                      </div>
                      {#if docsUrlFor(provider.provider)}
                        <a
                          href={docsUrlFor(provider.provider)}
                          target="_blank"
                          rel="noopener noreferrer"
                          class="shrink-0 text-[11px] font-medium text-primary hover:underline"
                        >
                          Get API Key ↗
                        </a>
                      {/if}
                    </div>

                  </div>
                {/each}
              </div>
            </div>
          {/each}
        </div>
      {/if}

      <div class="px-3 py-1.5 text-[11px] text-muted-foreground border-t border-border/30 bg-muted/20 -mx-4 sm:-mx-6 -mb-4 mt-4 flex items-center justify-between">
        <span>Env first → stored file · Local servers: <code class="font-mono">.mjolnr/providers/lm-studio.url</code></span>
      </div>
    </div>

    <Dialog.Footer class="flex flex-wrap items-center justify-end gap-2 border-t border-border/50 px-6 py-3 shrink-0">
      <Button variant="outline" size="sm" class="gap-1.5 text-xs font-semibold" disabled={refreshing} onclick={handleRefresh}>
        <HugeiconsIcon icon={RefreshIcon} strokeWidth={2} class={refreshing ? 'size-3.5 animate-spin' : 'size-3.5'} />
        <span>{refreshing ? 'Checking…' : 'Check & Refresh'}</span>
      </Button>
      <Button size="sm" class="text-xs font-semibold" onclick={() => (open = false)}>Done</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
