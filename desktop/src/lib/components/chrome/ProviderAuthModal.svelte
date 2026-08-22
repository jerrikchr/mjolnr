<!--
  ProviderConnectionsSurface: persistent connections view — providers and cloud
  integrations. Searchable, grouped provider cards with inline auth forms.
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
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import StatusOrb from './StatusOrb.svelte';

  let snap = $derived(clientStore.snapshot);
  let refreshing = $state(false);
  let query = $state('');
  let connectingProvider = $state<string | null>(null);
  let connectingError = $state<string | null>(null);
  let lmStudioAddress = $state('http://localhost:1234');
  let lmStudioToken = $state('');
  let apiKeyInput = $state('');
  let julesConnected = $state(false);
  let oauthCodeInput = $state('');
  let oauthPrompt = $state<{ provider: string; url: string; userCode?: string } | null>(null);

  onMount(() => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
    // Browser-mode tests can expose partial Tauri internals without the
    // callback bridge. A missing bridge is unavailable integration state, not
    // a reason to create an unhandled rejection during surface rendering.
    void listen<{ provider: string; url: string; userCode?: string }>('mjolnr-oauth-authorize', (event) => {
      oauthPrompt = event.payload;
    }).catch(() => undefined);
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
      case 'opencode-zen': return 'https://opencode.ai';
      case 'opencode-go': return 'https://opencode.ai';
      default: return '';
    }
  }

  function connectedCountFor(provider: string, models: ClientModel[]): number {
    return models.filter((m) => m.provider.toLowerCase() === provider.toLowerCase()).length;
  }

  type ClientModel = { provider: string; model: string; displayName: string };

  const providerCatalog = ['openai-codex', 'anthropic', 'gemini-cli', 'antigravity', 'openai', 'gemini', 'openrouter', 'opencode-zen', 'opencode-go', 'ollama', 'lm-studio'];
  let cards: Card[] = $derived([
    ...providerCatalog.map((provider) => {
      const account = snap.accounts.find((candidate) => candidate.provider === provider);
      return {
        provider,
        state: account?.state ?? 'disconnected',
        detail: account?.detail,
        models: snap.models.filter((model) => model.provider.toLowerCase() === provider).map((model) => model.model),
        kind: cardKind(account?.state ?? 'disconnected')
      };
    }),
    {
      provider: 'jules',
      state: julesConnected ? 'connected' : 'disconnected',
      models: [],
      kind: 'cloudAgent' as const
    }
  ]);

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
    oauthCodeInput = '';
    oauthPrompt = null;
  }

  function isGoogleOAuth(provider: string): boolean {
    return provider === 'gemini-cli' || provider === 'antigravity';
  }

  function isOAuthProvider(provider: string): boolean {
    return provider === 'anthropic' || provider === 'openai-codex' || isGoogleOAuth(provider);
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

  async function doCodexOAuth() {
    connectingError = null;
    const result = await clientStore.authCodexOAuth();
    if (result === true) {
      connectingProvider = null;
      oauthPrompt = null;
      await handleRefresh();
    } else connectingError = result.error;
  }

  async function doAnthropicOAuthStart() {
    connectingError = null;
    const result = await clientStore.authAnthropicOAuthStart();
    if (result !== true) connectingError = result.error;
  }

  async function doAnthropicOAuthComplete() {
    connectingError = null;
    const result = await clientStore.authAnthropicOAuthComplete(oauthCodeInput);
    if (result === true) {
      connectingProvider = null;
      oauthPrompt = null;
      await handleRefresh();
    } else connectingError = result.error;
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

<div class="mx-auto flex w-full max-w-6xl flex-col gap-5 p-6">
      <div class="border-b border-border/50 pb-4">
      <div class="flex items-center justify-between gap-3">
        <h1 class="flex items-center gap-2.5 text-2xl font-bold tracking-tight text-foreground">
          <HugeiconsIcon icon={SparklesIcon} strokeWidth={2} class="size-5 text-primary" />
          <span>Connections</span>
        </h1>
        <Badge variant={connectedTotal > 0 ? 'default' : 'secondary'} class="font-mono text-xs shrink-0">
          {connectedTotal} connected · {cards.length} available
        </Badge>
      </div>
      <p class="mt-1 max-w-3xl text-sm text-muted-foreground">
        Connect model providers, cloud agents, and governed tools. Each connection keeps its own trust and execution boundary.
      </p>
      <div class="relative mt-3">
        <HugeiconsIcon icon={Search01Icon} strokeWidth={2} class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          placeholder="Filter connections · e.g. Jules, lm-studio, openai"
          class="h-8 pl-8 text-sm"
          bind:value={query}
        />
      </div>
      </div>

    <div class="min-h-0 flex-1 overflow-y-auto">
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
                        {:else if isOAuthProvider(provider.provider)}
                          <p class="text-[11px] font-medium text-foreground">Subscription OAuth</p>
                          <p class="text-[11px] text-muted-foreground">Use the provider account already included with your subscription.</p>
                          <Button variant="outline" size="sm" class="h-6 text-[10px] gap-1 px-2" onclick={() => startConnect(provider.provider)}>
                            <HugeiconsIcon icon={PlugSocketIcon} strokeWidth={2} class="size-3" />
                            Connect with OAuth
                          </Button>
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
                        {#if isOAuthProvider(provider.provider)}
                          <p class="text-[11px] font-medium text-foreground">
                            {provider.provider === 'anthropic' ? 'Claude subscription login' : provider.provider === 'openai-codex' ? 'Codex subscription login' : 'Google account login'}
                          </p>
                          {#if oauthPrompt?.provider === provider.provider && provider.provider === 'openai-codex'}
                            <p class="text-[11px] text-muted-foreground">A browser window is open. Enter this one-time code:</p>
                            <code class="block rounded border border-border/60 bg-background px-2 py-1.5 font-mono text-xs font-semibold text-foreground">{oauthPrompt.userCode}</code>
                          {:else if oauthPrompt?.provider === provider.provider && provider.provider === 'anthropic'}
                            <p class="text-[11px] text-muted-foreground">Authorize in the browser, then paste the code shown on the final Claude page.</p>
                            <Input placeholder="Paste code or code#state" class="h-7 text-xs" bind:value={oauthCodeInput} />
                          {:else if oauthPrompt?.provider === provider.provider}
                            <p class="text-[11px] text-muted-foreground">A browser window is open. Waiting for the OAuth callback…</p>
                          {/if}
                        {:else if provider.provider === 'lm-studio'}
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
                            onclick={provider.provider === 'lm-studio'
                              ? doLmStudioConnect
                              : provider.provider === 'openai-codex'
                                ? doCodexOAuth
                                : provider.provider === 'anthropic'
                                  ? oauthPrompt?.provider === 'anthropic' ? doAnthropicOAuthComplete : doAnthropicOAuthStart
                                  : isGoogleOAuth(provider.provider) ? () => doGoogleOAuth(provider.provider) : () => doApiKeyConnect(provider.provider)}
                          >
                            <HugeiconsIcon icon={ArrowRight02Icon} strokeWidth={2} class="size-3" />
                            {provider.provider === 'lm-studio'
                              ? 'Connect LM Studio'
                              : provider.provider === 'openai-codex'
                                ? 'Open Codex login'
                                : provider.provider === 'anthropic'
                                  ? oauthPrompt?.provider === 'anthropic' ? 'Complete Claude login' : 'Open Claude login'
                                  : isGoogleOAuth(provider.provider) ? 'Continue with Google' : 'Save Key'}
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

      <div class="px-3 py-2 text-[11px] text-muted-foreground border-t border-border/30 bg-muted/20 -mb-1 mt-4 flex items-center justify-between">
        <span>Env first → stored file · Local servers: <code class="font-mono">.mjolnr/providers/lm-studio.url</code></span>
      </div>
    </div>

    <div class="flex flex-wrap items-center justify-end gap-2 border-t border-border/50 pt-4">
      <Button variant="outline" size="sm" class="gap-1.5 text-xs font-semibold" disabled={refreshing} onclick={handleRefresh}>
        <HugeiconsIcon icon={RefreshIcon} strokeWidth={2} class={refreshing ? 'size-3.5 animate-spin' : 'size-3.5'} />
        <span>{refreshing ? 'Checking…' : 'Check & Refresh'}</span>
      </Button>
    </div>
</div>
