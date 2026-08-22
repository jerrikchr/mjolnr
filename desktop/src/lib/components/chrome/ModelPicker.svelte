<script lang="ts">
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import { Search01Icon } from '@hugeicons/core-free-icons';
  import { Input } from '$lib/components/ui/input';
  import * as Select from '$lib/components/ui/select';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';

  type ModelChoice = { provider: string; model: string; displayName: string };

  let {
    models = [],
    value = $bindable(''),
    onopenproviderauth,
    onselect
  }: {
    models: ModelChoice[];
    value: string;
    onopenproviderauth?: () => void;
    onselect?: (choice: ModelChoice) => void;
  } = $props();

  let query = $state('');

  // Item values carry their group (`provider::model`) so the same model id
  // offered by two providers — routine now that full subscription catalogs
  // overlap — resolves to exactly the row that was picked. The `value` prop
  // stays a bare model id; this component owns the translation in both
  // directions.
  const SEPARATOR = '::';
  function wireValue(choice: ModelChoice): string {
    return `${choice.provider}${SEPARATOR}${choice.model}`;
  }
  function choiceForWire(wire: string): ModelChoice | undefined {
    const index = wire.indexOf(SEPARATOR);
    if (index === -1) return undefined;
    const provider = wire.slice(0, index);
    const model = wire.slice(index + SEPARATOR.length);
    return models.find((m) => m.provider === provider && m.model === model);
  }

  // Group by provider; locals first (Orca local-first: accounts panes surface
  // local / system-default affordances before remote), then alpha.
  let groups = $derived((() => {
    const needle = query.trim().toLowerCase();
    const filtered = needle
      ? models.filter((m) =>
          `${m.provider} ${m.model} ${m.displayName}`.toLowerCase().includes(needle)
        )
      : models;

    const order = (provider: string) => {
      if (provider === 'lm-studio' || provider === 'ollama' || provider === 'vllm') return `0-${provider}`;
      return `1-${provider}`;
    };

    const map = new Map<string, ModelChoice[]>();
    for (const choice of filtered) {
      const bucket = map.get(choice.provider) ?? [];
      bucket.push(choice);
      map.set(choice.provider, bucket);
    }
    return Array.from(map.entries()).sort((a, b) => order(a[0]).localeCompare(order(b[0])));
  })());

  let labelForValue = $derived(
    models.find((m) => m.model === value)?.displayName ?? (value || 'Select model')
  );

  // The bound value is a bare model id; translate it to the composite wire
  // value the items use so highlight/selection state survives duplicates.
  let selectedWire = $derived.by(() => {
    const hit = models.find((m) => m.model === value);
    return hit ? wireValue(hit) : value;
  });

  // The parent synchronized on every select change so choosing a Gemini model
  // cannot leave the previous Codex provider paired with it. Echo safety is
  // now structural: programmatic writes to `value` re-enter here carrying a
  // composite of exactly the snapshot's route, and the parent treats an
  // identical-route pick as a no-op, so truth-sync can never re-dispatch
  // truth as a command.
  function handleSelect(wire: string) {
    const choice = choiceForWire(wire);
    if (!choice) return;
    value = choice.model;
    onselect?.(choice);
  }
</script>

<Select.Root type="single" value={selectedWire} onValueChange={handleSelect}>
  <Select.Trigger class="h-6.5 border-border/60 bg-muted/40 text-[11px] px-2 py-0 gap-1 rounded-md font-mono hover:bg-muted/70 hover:text-foreground min-w-36">
    <span class="text-primary font-sans font-medium">⚡</span>
    <span class="truncate max-w-40">{labelForValue}</span>
  </Select.Trigger>
  <Select.Content class="max-h-[420px] w-[520px] p-0 overflow-hidden">
    <div class="sticky top-0 z-10 border-b border-border/60 bg-popover p-2">
      <div class="relative">
        <HugeiconsIcon icon={Search01Icon} strokeWidth={2} class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          placeholder="Filter models… (provider, model, or display name)"
          class="h-7 pl-8 text-xs"
          bind:value={query}
          onkeydown={(e) => {
            // bits-ui's select traps arrow keys; keep typing honest.
            if (e.key === 'Escape') query = '';
            e.stopPropagation();
          }}
        />
      </div>
      <p class="mt-1.5 text-[11px] text-muted-foreground">
        {#if query.trim()}
          {groups.reduce((n, [, list]) => n + list.length, 0)} / {models.length} matches
        {:else}
          {models.length} model{models.length === 1 ? '' : 's'} · grouped by provider
        {/if}
      </p>
    </div>

    <div class="max-h-[320px] overflow-y-auto py-1">
      {#if groups.length === 0}
        <div class="px-3 py-6 text-center">
          <p class="text-sm text-muted-foreground">No model matches that filter.</p>
          {#if onopenproviderauth}
            <Button variant="outline" size="sm" class="mt-3 gap-1 text-xs" onclick={onopenproviderauth}>
              Connect a provider
            </Button>
          {/if}
        </div>
      {:else}
        {#each groups as [provider, list] (provider)}
          <Select.Group>
            <Select.Label class="flex items-center justify-between px-2 py-1.5 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground sticky top-0 bg-popover/95 backdrop-blur">
              <span>{provider}</span>
              <Badge variant="secondary" class="h-4 px-1.5 text-[10px] leading-none">{list.length}</Badge>
            </Select.Label>
            {#each list as choice (wireValue(choice))}
              <Select.Item value={wireValue(choice)} label={choice.displayName} class="py-1.5">
                <div class="flex min-w-0 flex-col gap-0.5">
                  <span class="truncate font-mono text-xs">{choice.model}</span>
                  {#if choice.displayName !== choice.model}
                    <span class="truncate text-[11px] text-muted-foreground">{choice.displayName}</span>
                  {/if}
                </div>
              </Select.Item>
            {/each}
          </Select.Group>
        {/each}
      {/if}
    </div>

    <div class="border-t border-border/60 bg-muted/20 px-3 py-2 text-[11px] text-muted-foreground flex items-center justify-between">
      <span>LM Studio local models appear here when the server is running.</span>
      {#if onopenproviderauth}
        <button type="button" class="text-primary hover:underline font-medium" onclick={onopenproviderauth}>Connect provider →</button>
      {/if}
    </div>
  </Select.Content>
</Select.Root>
