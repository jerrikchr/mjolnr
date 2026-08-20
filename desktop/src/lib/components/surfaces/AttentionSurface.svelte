<script lang="ts">
  import { clientStore } from '$lib/runtime/client.svelte';
  import * as Alert from '$lib/components/ui/alert';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import * as Empty from '$lib/components/ui/empty';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import { CheckmarkCircle02Icon } from '@hugeicons/core-free-icons';
  import type {
    ClientApprovalDecision,
    ClientRecoveryDecision,
    ClientResumeChoice
  } from '$lib/runtime/contract';

  interface AttentionItem {
    id: string;
    state: 'attention' | 'approval' | 'recovery' | 'failed' | 'uncertain' | 'idle';
    title: string;
    detail: string;
    priority: number;
  }

  let snap = $derived(clientStore.snapshot);
  let isConnected = $derived(clientStore.connected);
  let lastError = $derived(clientStore.lastError);
  let resyncCount = $derived(clientStore.resyncCount);

  let items = $derived.by<AttentionItem[]>(() => {
    const next: AttentionItem[] = [];

    if (snap.storeFailure) {
      next.push({
        id: 'store',
        state: 'failed',
        title: 'Store failure recorded',
        detail: snap.storeFailure,
        priority: 1
      });
    }

    if (snap.recovery.state === 'required') {
      next.push({
        id: 'recovery',
        state: 'recovery',
        title: 'Recovery required',
        detail: snap.recovery.summary,
        priority: 2
      });
    }

    if (snap.pendingApproval) {
      next.push({
        id: 'approval',
        state: 'approval',
        title: 'Human approval required',
        detail: `${snap.pendingApproval.toolName} (${snap.pendingApproval.tier})`,
        priority: 3
      });
    }

    if (snap.resumeAdvice) {
      next.push({
        id: 'resume',
        state: 'attention',
        title: 'Resume choice required',
        detail: `${snap.resumeAdvice.warning} · estimated full resume tokens: ${snap.resumeAdvice.estimatedFullResumeTokens}`,
        priority: 4
      });
    }

    if (!isConnected) {
      next.push({
        id: 'bridge',
        state: 'uncertain',
        title: 'Desktop bridge unavailable',
        detail: lastError || 'Tauri IPC is unavailable in this environment.',
        priority: 5
      });
    }

    if (resyncCount > 0) {
      next.push({
        id: 'resync',
        state: 'attention',
        title: 'Updates were resynced',
        detail: `${resyncCount} update${resyncCount === 1 ? '' : 's'} were replayed from a snapshot.`,
        priority: 6
      });
    }

    if (!snap.session && snap.sessions.length > 0) {
      next.push({
        id: 'resume-session',
        state: 'idle',
        title: 'No active session selected',
        detail: 'Resume an existing session or create a new one to continue work.',
        priority: 7
      });
    }

    return next.sort((left, right) => left.priority - right.priority);
  });

  function resolveApproval(decision: ClientApprovalDecision) {
    if (!snap.pendingApproval) return;
    void clientStore.dispatch({
      type: 'resolveApproval',
      approval: snap.pendingApproval.id,
      decision
    });
  }

  function resolveRecovery(decision: ClientRecoveryDecision) {
    void clientStore.dispatch({ type: 'resolveRecovery', decision });
  }

  function resolveResume(choice: ClientResumeChoice) {
    void clientStore.dispatch({ type: 'resolveResume', choice });
  }

  function itemVariant(item: AttentionItem) {
    return item.state === 'failed' ? 'destructive' : 'outline';
  }

  // Governance-state colour for the card border/badge -- attention and
  // approval both read as the mockup's amber "needs a human" state; failed
  // stays the refusal red; recovery/uncertain get the proposal blue since
  // they are a decision, not yet a verdict.
  function itemBorderClass(item: AttentionItem) {
    if (item.state === 'failed') return 'border-gov-refusal-border';
    if (item.state === 'approval' || item.state === 'attention') return 'border-gov-approval-border';
    if (item.state === 'recovery' || item.state === 'uncertain') return 'border-gov-proposal-border';
    return '';
  }

  function itemBadgeClass(item: AttentionItem) {
    if (item.state === 'failed') return 'border-gov-refusal-border bg-gov-refusal-bg text-gov-refusal';
    if (item.state === 'approval' || item.state === 'attention')
      return 'border-gov-approval-border bg-gov-approval-bg text-gov-approval';
    if (item.state === 'recovery' || item.state === 'uncertain')
      return 'border-gov-proposal-border bg-gov-proposal-bg text-gov-proposal';
    return '';
  }
</script>

<div class="flex h-full flex-col gap-4 overflow-y-auto p-4" data-testid="attention-surface">
  <header class="flex flex-col justify-between gap-3 sm:flex-row sm:items-start">
    <div class="flex flex-col gap-1">
      <h1 class="text-2xl font-semibold tracking-tight">Attention</h1>
      <p class="text-muted-foreground text-sm">
        Prioritized from explicit snapshot truth only: durability failures, recovery, approval, resume, connection, resync, then session selection.
      </p>
    </div>
    <Badge variant={items.length > 0 ? itemVariant(items[0]) : 'secondary'}>
      {items.length > 0 ? 'Needs attention' : 'No blockers'}
    </Badge>
  </header>

  {#if items.length === 0}
    <Empty.Root class="border border-dashed">
      <Empty.Header>
        <Empty.Media variant="icon">
          <HugeiconsIcon icon={CheckmarkCircle02Icon} strokeWidth={2} />
        </Empty.Media>
        <Empty.Title>No blocking attention items</Empty.Title>
        <Empty.Description>
          The current snapshot does not require recovery, approval, resume resolution, or other immediate intervention.
        </Empty.Description>
      </Empty.Header>
    </Empty.Root>
  {:else}
    <div class="flex flex-col gap-3">
      {#each items as item (item.id)}
        {#if item.id === 'store'}
          <Alert.Root variant="destructive">
            <Alert.Title>{item.title}</Alert.Title>
            <Alert.Description>
              <div class="flex flex-col gap-2">
                <Badge variant="destructive">Priority {item.priority}</Badge>
                <p>{item.detail}</p>
              </div>
            </Alert.Description>
          </Alert.Root>
        {:else}
          <Card.Root class={itemBorderClass(item)}>
            <Card.Header class="flex-row items-start justify-between">
              <div class="flex flex-col gap-1">
                <Card.Title>{item.title}</Card.Title>
                <Card.Description>{item.detail}</Card.Description>
              </div>
              <Badge variant="outline" class={itemBadgeClass(item)}>Priority {item.priority}</Badge>
            </Card.Header>
            {#if item.id === 'approval' && snap.pendingApproval}
              <Card.Content class="flex flex-col gap-3">
                <pre class="bg-muted overflow-x-auto rounded-md p-3 text-xs whitespace-pre-wrap">{snap.pendingApproval.preview}</pre>
                <div class="flex flex-wrap gap-2">
                  <Button size="sm" onclick={() => resolveApproval('approve-once')}>Approve once</Button>
                  <Button variant="secondary" size="sm" onclick={() => resolveApproval('approve-exact-for-session')}>
                    Approve exact command for session
                  </Button>
                  <Button variant="destructive" size="sm" onclick={() => resolveApproval('deny')}>Deny</Button>
                </div>
              </Card.Content>
            {/if}

            {#if item.id === 'recovery' && snap.recovery.state === 'required'}
              <Card.Footer class="flex-wrap gap-2">
                <Button variant="secondary" size="sm" onclick={() => resolveRecovery('abandon-and-continue')}>
                  Abandon and continue
                </Button>
                <Button variant="destructive" size="sm" onclick={() => resolveRecovery('end-session')}>
                  End session
                </Button>
              </Card.Footer>
            {/if}

            {#if item.id === 'resume' && snap.resumeAdvice}
              <Card.Footer class="flex-wrap gap-2">
                <Button size="sm" onclick={() => resolveResume('compact')}>Compact resume</Button>
                {#if snap.resumeAdvice.hasHandoff}
                  <Button variant="secondary" size="sm" onclick={() => resolveResume('new-from-handoff')}>
                    Resume from handoff
                  </Button>
                {/if}
                <Button variant="outline" size="sm" onclick={() => resolveResume('full')}>Full resume</Button>
              </Card.Footer>
            {/if}
          </Card.Root>
        {/if}
      {/each}
    </div>
  {/if}
</div>
