<!--
  The mockup's governance modal: Council / Routes / Soul tabs, opened from
  the header's governance button and the sidebar's Persona row.

  All three tabs render runtime-owned projections. The Council tab shows the
  latest completed advisory distribution when one exists and live Fleet
  activity while a convocation runs. It never renders quorum, voting, or
  approval claims: the council informs a human and does not authorize work.
-->
<script lang="ts">
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import { BotIcon, MaskTheater01Icon, RouteIcon } from '@hugeicons/core-free-icons';
  import { clientStore } from '$lib/runtime/client.svelte';
  import * as Dialog from '$lib/components/ui/dialog';
  import * as Tabs from '$lib/components/ui/tabs';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import StatusOrb from './StatusOrb.svelte';

  let {
    open = $bindable(false),
    activeTab = $bindable('council'),
    onamendment
  }: {
    open?: boolean;
    activeTab?: string;
    onamendment?: (path: string, text: string) => void;
  } = $props();

  let snap = $derived(clientStore.snapshot);
  let composing = $state(false);

  // An amendment can only be composed from findings a human accepted. With
  // none accepted there is nothing to fold in, and offering the button anyway
  // would imply the council's own output was the input.
  let acceptedCount = $derived(
    snap.council?.findings.filter((finding) => finding.disposition?.disposition === 'accept')
      .length ?? 0
  );

  async function disposition(
    reviewId: string,
    findingId: string,
    value: 'accept' | 'reject' | 'defer'
  ): Promise<void> {
    await clientStore.dispatch({
      type: 'resolveCouncilFinding',
      reviewId,
      findingId,
      disposition: value
    });
  }

  async function composeAmendment(reviewId: string): Promise<void> {
    composing = true;
    try {
      await clientStore.dispatch({ type: 'proposeCouncilAmendment', reviewId });
    } finally {
      composing = false;
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="grid max-h-[80vh] w-full max-w-3xl grid-rows-[auto_auto_1fr] gap-0 overflow-hidden p-0 sm:max-w-3xl">
    <Dialog.Header class="flex-row items-center gap-2.5 border-b px-5 py-3.5">
      <HugeiconsIcon icon={RouteIcon} strokeWidth={2} class="size-4.5 text-primary" />
      <Dialog.Title class="text-base">Workspace Governance</Dialog.Title>
      <span class="font-mono text-xs text-muted-foreground">.mjolnr/</span>
    </Dialog.Header>

    <Tabs.Root bind:value={activeTab} class="contents">
      <div class="border-b px-3">
        <Tabs.List class="bg-transparent">
          <Tabs.Trigger value="council">Advisory Council</Tabs.Trigger>
          <Tabs.Trigger value="routes">Model &amp; Role Routes</Tabs.Trigger>
          <Tabs.Trigger value="soul">SOUL.md &amp; Personas</Tabs.Trigger>
        </Tabs.List>
      </div>

      <div class="overflow-y-auto p-5">
        <Tabs.Content value="council" class="flex flex-col gap-3">
          {#if snap.council}
            <div class="flex flex-col gap-1.5">
              <div class="flex items-center justify-between gap-3">
                <span class="text-xs font-medium uppercase tracking-wide text-muted-foreground">Latest advisory distribution</span>
                <Badge variant="secondary">{snap.council.roundsConducted} round{snap.council.roundsConducted === 1 ? '' : 's'}</Badge>
              </div>
              <p class="rounded-md border bg-muted/30 px-3.5 py-2.5 text-sm">{snap.council.question}</p>
              {#if snap.council.artifact}
                <p class="rounded-md border border-dashed px-3.5 py-2 text-xs text-muted-foreground">
                  Artifact: <code class="font-mono">{snap.council.artifact.path}</code>
                  <span class="ml-1">· source {snap.council.artifact.sourceDigest.slice(0, 12)}</span>
                </p>
              {/if}
            </div>

            {#if snap.council.findings.length > 0}
              <div class="flex flex-col gap-2">
                <span class="text-xs font-medium uppercase tracking-wide text-muted-foreground">Findings for human disposition</span>
                {#each snap.council.findings as finding (finding.id)}
                  <article class="rounded-md border bg-card px-3.5 py-3">
                    <div class="flex items-center justify-between gap-3">
                      <div class="flex min-w-0 flex-col">
                        <span class="text-sm font-medium">{finding.title}</span>
                        <span class="text-xs text-muted-foreground">Section: {finding.section}</span>
                      </div>
                      <Badge variant="secondary">{finding.disposition?.disposition ?? 'pending'}</Badge>
                    </div>
                    <div class="mt-3 flex flex-col gap-2">
                      {#each finding.positions as position (`${finding.id}-${position.role}`)}
                        <div class="rounded border bg-muted/20 px-3 py-2">
                          <span class="text-xs font-medium text-muted-foreground">{position.role}</span>
                          <p class="mt-1 whitespace-pre-wrap text-sm">{position.response}</p>
                          {#if position.critique}
                            <p class="mt-1 whitespace-pre-wrap text-xs text-muted-foreground">Dissent: {position.critique}</p>
                          {/if}
                        </div>
                      {/each}
                    </div>
                    <div class="mt-3 flex flex-wrap gap-2">
                      <Button size="sm" onclick={() => void disposition(snap.council?.reviewId ?? '', finding.id, 'accept')}>Accept finding</Button>
                      <Button size="sm" variant="secondary" onclick={() => void disposition(snap.council?.reviewId ?? '', finding.id, 'defer')}>Defer</Button>
                      <Button size="sm" variant="destructive" onclick={() => void disposition(snap.council?.reviewId ?? '', finding.id, 'reject')}>Reject finding</Button>
                    </div>
                  </article>
                {/each}
              </div>
            {/if}

            <div class="flex flex-col gap-2">
              {#each snap.council.contributions as contribution (contribution.role)}
                <article class="rounded-md border bg-card px-3.5 py-3">
                  <div class="mb-2 flex items-center gap-2">
                    <HugeiconsIcon icon={BotIcon} strokeWidth={2} class="size-3.5 text-muted-foreground" />
                    <span class="text-sm font-medium">{contribution.role}</span>
                  </div>
                  <p class="whitespace-pre-wrap text-sm">{contribution.proposal}</p>
                  {#if contribution.critique}
                    <div class="mt-3 border-t pt-2.5">
                      <span class="text-xs font-medium text-muted-foreground">Dissent / critique</span>
                      <p class="mt-1 whitespace-pre-wrap text-sm">{contribution.critique}</p>
                    </div>
                  {/if}
                </article>
              {/each}
            </div>

            {#if snap.council.artifact}
              <section class="rounded-md border bg-card px-3.5 py-3">
                <div class="mb-2 flex flex-wrap items-center gap-2">
                  <span class="text-sm font-medium">Amended artifact</span>
                  <Badge variant="secondary">{acceptedCount} accepted</Badge>
                </div>
                <p class="text-xs text-muted-foreground">
                  mjolnr marks the artifact up with the findings you accepted. It does not rewrite
                  the prose and it does not write the file — the draft opens in the editor as
                  unsaved text, and saving it is the ordinary governed save.
                </p>
                <div class="mt-3 flex flex-wrap items-center gap-2">
                  <Button
                    size="sm"
                    disabled={acceptedCount === 0 || composing}
                    onclick={() => void composeAmendment(snap.council?.reviewId ?? '')}
                  >
                    {composing ? 'Composing…' : 'Compose amended artifact'}
                  </Button>
                  {#if snap.council.amendment}
                    <Button
                      size="sm"
                      variant="secondary"
                      onclick={() =>
                        onamendment?.(
                          snap.council?.amendment?.path ?? '',
                          snap.council?.amendment?.text ?? ''
                        )}
                    >
                      Open draft in editor
                    </Button>
                  {/if}
                </div>
                {#if acceptedCount === 0}
                  <p class="mt-2 text-xs text-muted-foreground">
                    Accept at least one finding first. Nothing is folded in on the council's own
                    authority.
                  </p>
                {/if}
                {#if snap.council.amendment}
                  <pre
                    class="mt-3 max-h-56 overflow-auto rounded-md border bg-muted/30 px-3 py-2 text-xs whitespace-pre-wrap">{snap
                      .council.amendment.text}</pre>
                {/if}
              </section>
            {/if}

            <p class="text-xs text-muted-foreground">
              Advisory only. Review and disposition of individual findings is not an approval and
              cannot authorize a tool or side effect. Composing an amendment records a proposal;
              it does not write to the workspace.
            </p>
          {:else if clientStore.fleet.length === 0}
            <p class="text-sm text-muted-foreground">
              No completed council review or live convocation is present in this session.
            </p>
          {:else}
            <p class="text-xs text-muted-foreground">
              Live convocation activity for this session. mjolnr does not yet distinguish a council
              seat from an ordinary subagent in this feed, so this is the same roster the sidebar
              Fleet section shows.
            </p>
            {#each clientStore.fleet as agent (agent.child)}
              <div class="flex items-center justify-between gap-3 rounded-md border bg-card px-3.5 py-2.5">
                <div class="flex min-w-0 items-center gap-2.5">
                  <StatusOrb state={agent.done ? 'idle' : 'active'} />
                  <div class="flex min-w-0 flex-col">
                    <span class="truncate text-sm font-medium">{agent.latest}</span>
                    <span class="font-mono text-xs text-muted-foreground">{agent.short}</span>
                  </div>
                </div>
                <Badge variant={agent.done ? 'secondary' : 'default'}>{agent.done ? 'settled' : 'active'}</Badge>
              </div>
            {/each}
          {/if}
        </Tabs.Content>

        <Tabs.Content value="routes" class="flex flex-col gap-3">
          {#if snap.routes.length === 0}
            <p class="text-sm text-muted-foreground">
              No routes are configured for this workspace (<code class="font-mono text-xs">.mjolnr/routes.toml</code>).
            </p>
          {:else}
            <div class="overflow-x-auto rounded-md border">
              <table class="w-full text-sm">
                <thead>
                  <tr class="border-b bg-muted/50 text-left text-xs text-muted-foreground">
                    <th class="px-3 py-2 font-medium">Role / Route</th>
                    <th class="px-3 py-2 font-medium">Provider</th>
                    <th class="px-3 py-2 font-medium">Model</th>
                    <th class="px-3 py-2 font-medium">Persona</th>
                  </tr>
                </thead>
                <tbody>
                  {#each snap.routes as route (route.name)}
                    <tr class="border-b last:border-0">
                      <td class="px-3 py-2">
                        <div class="flex flex-col">
                          <span class="font-medium">{route.name}</span>
                          {#if route.roles.length > 0}
                            <span class="font-mono text-xs text-muted-foreground">{route.roles.join(', ')}</span>
                          {/if}
                        </div>
                      </td>
                      <td class="px-3 py-2">
                        <div class="flex items-center gap-1.5">
                          <HugeiconsIcon icon={RouteIcon} strokeWidth={2} class="size-3.5 text-muted-foreground" />
                          {route.provider}
                        </div>
                      </td>
                      <td class="px-3 py-2 font-mono text-xs">{route.model}</td>
                      <td class="px-3 py-2 text-muted-foreground">{route.persona ?? '—'}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        </Tabs.Content>

        <Tabs.Content value="soul" class="flex flex-col gap-4">
          <div class="flex flex-col gap-1.5">
            <span class="text-xs font-medium text-muted-foreground">Active persona</span>
            <div class="flex items-center gap-2 text-sm">
              <HugeiconsIcon icon={MaskTheater01Icon} strokeWidth={2} class="size-4 text-muted-foreground" />
              {snap.activePersona ?? 'Route default'}
            </div>
          </div>

          {#if snap.personas.length > 0}
            <div class="flex flex-col gap-1.5">
              <span class="text-xs font-medium text-muted-foreground">Available personas</span>
              <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
                {#each snap.personas as persona (persona.name)}
                  <div
                    class="rounded-md border px-3 py-2 text-center text-sm {persona.name === snap.activePersona
                      ? 'border-accent-border bg-accent-muted text-accent-bright'
                      : 'text-muted-foreground'}"
                  >
                    {persona.name}
                  </div>
                {/each}
              </div>
            </div>
          {/if}

          {#if snap.souls.length > 0}
            <div class="flex flex-col gap-1.5">
              <span class="text-xs font-medium text-muted-foreground">Soul files in effect</span>
              <ul class="flex flex-col gap-1">
                {#each snap.souls as soul (soul)}
                  <li class="flex items-center gap-1.5 font-mono text-xs">
                    <HugeiconsIcon icon={BotIcon} strokeWidth={2} class="size-3.5 text-muted-foreground" />
                    {soul}
                  </li>
                {/each}
              </ul>
            </div>
          {/if}

          {#if snap.personas.length === 0 && snap.souls.length === 0}
            <p class="text-sm text-muted-foreground">
              No personas or Soul files were discovered under <code class="font-mono text-xs">.mjolnr/personas/</code>.
            </p>
          {/if}

          <p class="text-xs text-muted-foreground">
            Editing SOUL.md content and switching persona from the desktop client are not wired
            yet — this tab is read-only. Use <code class="font-mono text-xs">/persona</code> in the
            terminal client to change it live.
          </p>
        </Tabs.Content>
      </div>
    </Tabs.Root>
  </Dialog.Content>
</Dialog.Root>
