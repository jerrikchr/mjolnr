<script lang="ts">
  import { clientStore } from '$lib/runtime/client.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import * as Empty from '$lib/components/ui/empty';
  import { ScrollArea } from '$lib/components/ui/scroll-area';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import { Activity01Icon, Cancel01Icon } from '@hugeicons/core-free-icons';

  interface Props {
    onclose?: () => void;
  }

  let { onclose }: Props = $props();

  let snap = $derived(clientStore.snapshot);
  let usage = $derived(snap.usage);
  let budget = $derived(snap.budget);
  let selectedSession = $derived(snap.session || 'None');
  let diagnostics = $derived(snap.contextDiagnostics ?? []);

  function policyVariant() {
    if (snap.policy === 'full-auto') return 'destructive';
    if (snap.policy === 'read-only') return 'secondary';
    return 'outline';
  }
</script>

<aside
  class="bg-background flex h-full w-full max-w-full flex-col border-t lg:max-w-[320px] lg:border-t-0 lg:border-l"
  data-testid="inspector-pane"
  aria-labelledby="inspector-pane-title"
>
  <header class="flex items-center justify-between border-b p-4">
    <div class="flex items-center gap-2">
      <HugeiconsIcon icon={Activity01Icon} strokeWidth={2} />
      <h2 id="inspector-pane-title" class="font-semibold">Inspector & Telemetry</h2>
    </div>
    {#if onclose}
      <Button variant="ghost" size="icon-sm" aria-label="Close Inspector" onclick={onclose}>
        <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} />
      </Button>
    {/if}
  </header>

  <ScrollArea class="min-h-0 flex-1">
    <div class="flex flex-col gap-4 p-4">
      <Card.Root>
        <Card.Header>
          <Card.Title>Active Session</Card.Title>
          <Card.Description class="font-mono">{selectedSession}</Card.Description>
        </Card.Header>
        <Card.Content>
          <dl class="grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-3 text-sm">
            <dt class="text-muted-foreground">Policy mode</dt>
            <dd class="justify-self-end"><Badge variant={policyVariant()}>{snap.policy}</Badge></dd>
            <dt class="text-muted-foreground">Provider / model</dt>
            <dd class="justify-self-end text-right">{snap.provider || 'Unset'} / {snap.model || 'Unset'}</dd>
          </dl>
        </Card.Content>
      </Card.Root>

      <Card.Root data-testid="context-diagnostics">
        <Card.Header>
          <Card.Title>Workspace diagnostics</Card.Title>
          <Card.Description>Rust-reported context and configuration checks.</Card.Description>
        </Card.Header>
        <Card.Content>
          {#if diagnostics.length === 0}
            <p class="text-muted-foreground text-sm">No context diagnostics reported.</p>
          {:else}
            <ul class="flex flex-col gap-2" aria-label="Workspace diagnostics">
              {#each diagnostics as diagnostic}
                <li class="rounded-md border border-destructive/40 bg-destructive/5 p-3">
                  <code class="font-mono text-xs text-destructive">{diagnostic.code}</code>
                  <p class="mt-1 text-sm">{diagnostic.detail}</p>
                </li>
              {/each}
            </ul>
          {/if}
        </Card.Content>
      </Card.Root>

      <Card.Root>
        <Card.Header>
          <Card.Title>Token Telemetry & Limits</Card.Title>
          <Card.Description>Runtime-reported usage and governed limits.</Card.Description>
        </Card.Header>
        <Card.Content>
          <dl class="grid grid-cols-[1fr_auto] gap-x-4 gap-y-3 text-sm">
            <dt class="text-muted-foreground">Input tokens</dt>
            <dd>{usage.inputTokens.toLocaleString()}</dd>
            <dt class="text-muted-foreground">Output tokens</dt>
            <dd>{usage.outputTokens.toLocaleString()}</dd>
            <dt class="text-muted-foreground">Turns spent</dt>
            <dd>{budget.providerTurns} / {budget.maxProviderTurns}</dd>
            <dt class="text-muted-foreground">Tool calls</dt>
            <dd>{budget.toolCalls} / {budget.maxToolCalls}</dd>
          </dl>
        </Card.Content>
      </Card.Root>

      <Card.Root>
        <Card.Header>
          <Card.Title>Session Fleet ({snap.sessions.length})</Card.Title>
          <Card.Description>Sessions reported by the runtime snapshot.</Card.Description>
        </Card.Header>
        <Card.Content>
          {#if snap.sessions.length === 0}
            <Empty.Root>
              <Empty.Header>
                <Empty.Title>No active sessions in fleet</Empty.Title>
                <Empty.Description>Open or create a session to see it here.</Empty.Description>
              </Empty.Header>
            </Empty.Root>
          {:else}
            <ul class="flex flex-col gap-2">
              {#each snap.sessions as session}
                <li class="rounded-md border p-3" data-active={session.id === snap.session}>
                  <div class="flex items-center justify-between gap-2">
                    <span class="text-sm font-medium">{session.title}</span>
                    <Badge variant={session.id === snap.session ? 'secondary' : 'outline'}>{session.status}</Badge>
                  </div>
                  <p class="text-muted-foreground mt-1 font-mono text-xs">{session.id.slice(0, 8)}...</p>
                </li>
              {/each}
            </ul>
          {/if}
        </Card.Content>
      </Card.Root>
    </div>
  </ScrollArea>
</aside>
