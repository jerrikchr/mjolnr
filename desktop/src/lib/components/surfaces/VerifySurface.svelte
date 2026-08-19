<script lang="ts">
  import { clientStore } from '$lib/runtime/client.svelte';
  import * as Alert from '$lib/components/ui/alert';
  import { Badge } from '$lib/components/ui/badge';
  import * as Card from '$lib/components/ui/card';
  import * as Empty from '$lib/components/ui/empty';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import { Alert02Icon, ClipboardCheckIcon } from '@hugeicons/core-free-icons';
  import type { ClientMessage } from '$lib/runtime/contract';

  type ToolMessage = Extract<ClientMessage, { kind: 'tool' }>;

  let snap = $derived(clientStore.snapshot);
  let toolMessages = $derived.by<ToolMessage[]>(() =>
    snap.messages.filter((message): message is ToolMessage => message.kind === 'tool')
  );
  let succeededCount = $derived(toolMessages.filter((message) => message.outcome === 'ok').length);
  let refusedCount = $derived(toolMessages.filter((message) => message.outcome === 'refused').length);
  let failedCount = $derived(toolMessages.filter((message) => message.outcome === 'failed').length);

  // Governance-state colour: ok/refused/failed map onto the same
  // --gov-verified/--gov-refusal tokens every other surface uses.
  function outcomeClass(outcome: ToolMessage['outcome']) {
    if (outcome === 'ok') return 'border-gov-verified-border bg-gov-verified-bg text-gov-verified';
    return 'border-gov-refusal-border bg-gov-refusal-bg text-gov-refusal';
  }

  function outcomeLabel(outcome: ToolMessage['outcome']) {
    if (outcome === 'ok') return 'Succeeded';
    if (outcome === 'refused') return 'Refused';
    return 'Failed';
  }
</script>

<div class="flex h-full flex-col gap-4 overflow-y-auto p-4" data-testid="verify-surface">
  <header class="flex flex-col justify-between gap-3 sm:flex-row sm:items-start">
    <div class="flex flex-col gap-1">
      <h1 class="text-2xl font-semibold tracking-tight">Verify</h1>
      <p class="text-muted-foreground text-sm">
        Evidence shown here comes only from explicit tool-result and store-failure fields already on the snapshot.
      </p>
    </div>
    <Badge
      variant="outline"
      class={failedCount > 0 || snap.storeFailure ? 'border-gov-refusal-border bg-gov-refusal-bg text-gov-refusal' : ''}
    >
      {snap.runActive ? 'Run active' : 'Idle'}
    </Badge>
  </header>

  <div class="grid gap-3 sm:grid-cols-3">
    <Card.Root>
      <Card.Header>
        <Card.Description>Succeeded tool results</Card.Description>
        <Card.Title class="text-2xl text-gov-verified">{succeededCount}</Card.Title>
      </Card.Header>
    </Card.Root>
    <Card.Root>
      <Card.Header>
        <Card.Description>Refused</Card.Description>
        <Card.Title class="text-2xl text-gov-refusal">{refusedCount}</Card.Title>
      </Card.Header>
    </Card.Root>
    <Card.Root>
      <Card.Header>
        <Card.Description>Failed</Card.Description>
        <Card.Title class="text-2xl text-gov-refusal">{failedCount}</Card.Title>
      </Card.Header>
    </Card.Root>
  </div>

  {#if snap.storeFailure}
    <Alert.Root variant="destructive">
      <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
      <Alert.Title>Store failure recorded</Alert.Title>
      <Alert.Description>{snap.storeFailure}</Alert.Description>
    </Alert.Root>
  {/if}

  <Card.Root>
    <Card.Header>
      <Card.Title>Current DTO gap</Card.Title>
      <Card.Description>
        The desktop contract still lacks explicit verification-command exit statuses, claim-to-check mapping, and a final verification verdict.
      </Card.Description>
    </Card.Header>
    <Card.Content>
      <p class="text-sm">
        This surface therefore reports only the bounded evidence the runtime already exposes. A successful tool result is not presented as verified work.
      </p>
    </Card.Content>
  </Card.Root>

  {#if toolMessages.length === 0}
    <Empty.Root class="border border-dashed">
      <Empty.Header>
        <Empty.Media variant="icon">
          <HugeiconsIcon icon={ClipboardCheckIcon} strokeWidth={2} />
        </Empty.Media>
        <Empty.Title>No explicit verification evidence yet</Empty.Title>
        <Empty.Description>
          No tool-result records are present in the current snapshot. Verification evidence will appear here once the runtime emits it explicitly.
        </Empty.Description>
      </Empty.Header>
    </Empty.Root>
  {:else}
    <div class="flex flex-col gap-3">
      {#each toolMessages as message (message.id)}
        <Card.Root>
          <Card.Header class="flex-row items-start justify-between">
            <div class="flex flex-col gap-1">
              <Card.Title>{message.name}</Card.Title>
              {#if message.reasonCode}
                <Card.Description>Reason code: <code>{message.reasonCode}</code></Card.Description>
              {/if}
            </div>
            <Badge variant="outline" class={outcomeClass(message.outcome)}>{outcomeLabel(message.outcome)}</Badge>
          </Card.Header>
          <Card.Content class="flex flex-col gap-2">
            <pre class="bg-muted overflow-x-auto rounded-md p-3 text-xs whitespace-pre-wrap">{message.detail}</pre>
            {#if message.detailTruncated}
              <p class="text-muted-foreground text-sm">Result output was truncated by the runtime contract.</p>
            {/if}
          </Card.Content>
        </Card.Root>
      {/each}
    </div>
  {/if}
</div>
