<!--
  ProviderAuthModal: Connect Provider surface — Orca-inspired.
  Searchable, grouped provider cards with LM Studio and the full
  openai-compat catalog. No hardcoded 5-provider list.
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
    CpuIcon,
    Search01Icon
  } from '@hugeicons/core-free-icons';
  import { clientStore } from '$lib/runtime/client.svelte';
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

  type Card = {
    provider: string;
    state: string;
    detail?: string;
    models: string[];
    kind: 'connected' | 'connecting' | 'needsAuth' | 'disconnected' | 'unavailable';
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

  function envVarFor(provider: string): string {
    if (provider === 'lm-studio') return 'LM_API_TOKEN · MJOLNR_LM_STUDIO_BASE_URL / .mjolnr/providers/lm-studio.url';
    if (provider === 'ollama') return 'OLLAMA_HOST (optional)';
    const name = provider.toUpperCase().replace(/-/g, '_');
    return `${name}_API_KEY`;
  }

  function descriptionFor(provider: string): string {
    switch (provider) {
      case 'anthropic': return 'Claude — subscription or API key';
      case 'openai-codex': return 'Codex — subscription login';
      case 'openai': return 'OpenAI API — GPT-4o, o3-mini, o1';
      case 'gemini': return 'Google Gemini — Gemini 2.5 Pro, Flash';
      case 'gemini-cli': return 'Gemini CLI — uses your local Gemini login';
      case 'antigravity': return 'Antigravity — uses your local Antigravity login';
      case 'openrouter': return 'OpenRouter — universal aggregator';
      case 'ollama': return 'Ollama — local server on localhost:11434';
      case 'lm-studio': return 'LM Studio — local server on localhost:1234 · models that support tools';
      case 'nvidia': return 'NVIDIA NIM';
      case 'xai': return 'xAI — Grok';
      case 'vercel-gateway': return 'Vercel AI Gateway';
      case 'cloudflare-gateway': return 'Cloudflare AI Gateway (needs base URL)';
      case 'deepseek': return 'DeepSeek';
      case 'mistral': return 'Mistral AI';
      case 'groq': return 'Groq';
      case 'together': return 'Together AI';
      case 'fireworks': return 'Fireworks AI';
      case 'perplexity': return 'Perplexity AI';
      case 'moonshot': return 'Moonshot / Kimi';
      case 'zhipu': return 'Zhipu / GLM';
      case 'qwen': return 'Qwen / DashScope';
      case 'huggingface': return 'Hugging Face';
      case 'tokenrouter': return 'TokenRouter';
      case 'vllm': return 'vLLM — local server on localhost:8000';
      case 'opencode-zen': return 'OpenCode Zen';
      case 'opencode-go': return 'OpenCode Go';
      default: return 'OpenAI-compatible endpoint';
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
    snap.accounts.map((account) => ({
      provider: account.provider,
      state: account.state,
      detail: account.detail,
      models: snap.models
        .filter((m) => m.provider.toLowerCase() === account.provider.toLowerCase())
        .map((m) => m.model),
      kind: cardKind(account.state)
    }))
  );

  let filtered = $derived(
    (() => {
      const needle = query.trim().toLowerCase();
      if (!needle) return cards;
      return cards.filter((card) =>
        `${card.provider} ${card.state} ${card.detail ?? ''} ${descriptionFor(card.provider)} ${envVarFor(card.provider)}`
          .toLowerCase()
          .includes(needle)
      );
    })()
  );

  let grouped = $derived((() => {
    const order: Record<Card['kind'], number> = { connected: 0, connecting: 1, needsAuth: 2, unavailable: 3, disconnected: 4 };
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

  function openGuidedSetup() {
    open = false;
    goto('/onboarding');
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-3xl max-h-[86vh] overflow-hidden bg-card border-border/80 shadow-2xl p-0 gap-0 flex flex-col">
    <Dialog.Header class="px-6 pt-6 pb-3 border-b border-border/50 shrink-0">
      <div class="flex items-center justify-between gap-3">
        <Dialog.Title class="flex items-center gap-2.5 text-lg font-bold text-foreground">
          <HugeiconsIcon icon={SparklesIcon} strokeWidth={2} class="size-5 text-primary" />
          <span>Connect Provider</span>
        </Dialog.Title>
        <div class="flex items-center gap-2 shrink-0">
          <Badge variant={connectedTotal > 0 ? 'default' : 'secondary'} class="font-mono text-xs">
            {connectedTotal} connected · {cards.length} available
          </Badge>
        </div>
      </div>
      <Dialog.Description class="text-xs text-muted-foreground mt-1">
        Providers come from the runtime registry. Credentials resolve from environment first, then stored file. Local servers (LM Studio, Ollama, vLLM) are workspace config under <code class="font-mono text-[11px]">.mjolnr/providers/lm-studio.url</code>.
      </Dialog.Description>
      <div class="relative mt-3">
        <HugeiconsIcon icon={Search01Icon} strokeWidth={2} class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          placeholder="Filter providers… (e.g. lm-studio, local, ollama)"
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
                          <p class="text-xs leading-relaxed text-muted-foreground">{descriptionFor(provider.provider)}</p>
                          {#if provider.detail}
                            <p class="mt-1 truncate text-[11px] text-muted-foreground">{provider.detail}</p>
                          {/if}
                        </div>
                      </div>
                      <div class="shrink-0 flex flex-col items-end gap-1">
                        {#if provider.kind === 'connected'}
                          <Badge variant="outline" class="gap-1 border-gov-verified-border bg-gov-verified-bg text-gov-verified text-[11px]">
                            <HugeiconsIcon icon={CheckmarkCircle02Icon} strokeWidth={2} class="size-3" />
                            Connected{count ? ` · ${count} model${count === 1 ? '' : 's'}` : ''}
                          </Badge>
                        {:else if provider.kind === 'connecting'}
                          <Badge variant="secondary" class="text-[11px]">Discovering…</Badge>
                        {:else if provider.kind === 'needsAuth'}
                          <Badge variant="destructive" class="text-[11px]">Needs Reauth</Badge>
                        {:else if provider.kind === 'unavailable'}
                          <Badge variant="destructive" class="text-[11px]">Unavailable</Badge>
                        {:else}
                          <Badge variant="secondary" class="text-[11px] text-muted-foreground">Not Connected</Badge>
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

                    <div class="flex flex-wrap items-center justify-between gap-2 border-t border-border/40 pt-2 text-xs">
                      <div class="flex min-w-0 items-center gap-1.5 font-mono text-[11px] text-muted-foreground">
                        <HugeiconsIcon icon={Key01Icon} class="size-3 shrink-0 text-muted-foreground" />
                        <span class="truncate">Env: <code class="rounded border border-border/60 bg-background/80 px-1.5 py-0.5 text-foreground">{envVarFor(provider.provider)}</code></span>
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

                    {#if provider.provider === 'lm-studio'}
                      <div class="rounded-lg border border-border/60 bg-muted/30 px-3 py-2 text-[11px] leading-relaxed text-muted-foreground">
                        <span class="font-medium text-foreground">LM Studio</span> — run LM Studio's Local Server, load a model that supports tool use, then
                        <code class="font-mono text-foreground">mjolnr auth login lm-studio</code> or set
                        <code class="font-mono text-foreground">MJOLNR_LM_STUDIO_BASE_URL</code>. Blank token is keyless (default).
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>
            </div>
          {/each}
        </div>
      {/if}

      <div class="mt-5 rounded-lg border border-border/60 bg-background/50 p-3 text-xs text-muted-foreground flex gap-2">
        <HugeiconsIcon icon={CpuIcon} class="size-3.5 text-primary mt-0.5 shrink-0" />
        <div class="space-y-1 leading-relaxed">
          <p class="font-medium text-foreground">How to authenticate</p>
          <p class="text-[11px]">
            Export API keys in your shell profile (<code class="font-mono text-foreground">export ANTHROPIC_API_KEY=...</code>) or launch mjolnr where keys are active. For local models the endpoint lives in the project: <code class="font-mono text-foreground">.mjolnr/providers/lm-studio.url</code>. CLI fallback: <code class="font-mono text-foreground">mjolnr auth login &lt;provider&gt;</code>.
          </p>
          <p class="text-[11px]">
            Subscription providers (Anthropic Claude, Codex, Gemini CLI, Antigravity) use OAuth: <code class="font-mono text-foreground">mjolnr auth login anthropic</code> etc.
          </p>
        </div>
      </div>
    </div>

    <Dialog.Footer class="flex flex-wrap items-center justify-between gap-3 border-t border-border/50 px-6 py-3 shrink-0">
      <Button variant="ghost" size="sm" class="text-xs text-muted-foreground hover:text-foreground" onclick={openGuidedSetup}>
        <span>Open full guided setup</span>
        <HugeiconsIcon icon={ArrowRight01Icon} strokeWidth={2} class="size-3.5 ml-1" />
      </Button>
      <div class="flex items-center gap-2">
        <Button variant="outline" size="sm" class="gap-1.5 text-xs font-semibold" disabled={refreshing} onclick={handleRefresh}>
          <HugeiconsIcon icon={RefreshIcon} strokeWidth={2} class={refreshing ? 'size-3.5 animate-spin' : 'size-3.5'} />
          <span>{refreshing ? 'Checking…' : 'Check & Refresh'}</span>
        </Button>
        <Button size="sm" class="text-xs font-semibold" onclick={() => (open = false)}>Done</Button>
      </div>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
