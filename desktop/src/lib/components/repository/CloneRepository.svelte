<script lang="ts">
  import { clientStore, type ClientRefusal } from '$lib/runtime/client.svelte';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';

  let source = $state('');
  let destination = $state('');
  let open = $state(false);
  let busy = $state(false);
  let refusal = $state<ClientRefusal | null>(null);

  function begin() {
    refusal = null;
    if (source.trim() && destination.trim()) open = true;
  }

  function close() {
    if (busy) return;
    open = false;
    refusal = null;
  }

  async function confirm() {
    if (busy || !source.trim() || !destination.trim()) return;
    busy = true;
    refusal = null;
    const result = await clientStore.dispatch({
      type: 'cloneProject',
      source: source.trim(),
      destination: destination.trim()
    });
    busy = false;
    if (result) {
      refusal = result;
      return;
    }
    close();
  }
</script>

<section class="flex flex-col gap-3 rounded-lg border bg-card p-4" data-testid="clone-repository">
  <div>
    <h3 class="font-medium">Clone a project</h3>
    <p class="mt-1 text-sm text-muted-foreground">
      Start from a repository URL or local source. smed will create a new folder, verify it, and
      make it the project root.
    </p>
  </div>

  <div class="grid gap-2 sm:grid-cols-2">
    <label class="flex flex-col gap-1 text-xs font-medium" for="clone-source">
      Repository source
      <Input id="clone-source" bind:value={source} placeholder="https://… or /path/to/repo" />
    </label>
    <label class="flex flex-col gap-1 text-xs font-medium" for="clone-destination">
      New project folder
      <Input id="clone-destination" bind:value={destination} placeholder="/path/to/new-project" />
    </label>
  </div>

  <Button class="w-fit" disabled={!source.trim() || !destination.trim()} onclick={begin}>
    Review clone
  </Button>

  <Dialog.Root bind:open>
    <Dialog.Content class="w-full max-w-xl">
      <Dialog.Header>
        <Dialog.Title>Review clone</Dialog.Title>
        <Dialog.Description>
          This operator-controlled action uses the network when the source is remote and creates
          files at the destination. smed will not overwrite an existing folder.
        </Dialog.Description>
      </Dialog.Header>

      <dl class="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-2 rounded-md border bg-muted/20 p-3 text-xs">
        <dt class="text-muted-foreground">Source</dt>
        <dd class="break-all font-mono">{source.trim()}</dd>
        <dt class="text-muted-foreground">Destination</dt>
        <dd class="break-all font-mono">{destination.trim()}</dd>
      </dl>

      {#if refusal}
        <div class="mt-3 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-xs" role="alert">
          <div class="font-mono">{refusal.code ?? 'REFUSED'}</div>
          <div>{refusal.message}</div>
        </div>
      {/if}

      <Dialog.Footer>
        <Button variant="outline" disabled={busy} onclick={close}>Cancel</Button>
        <Button data-testid="clone-confirm" disabled={busy} onclick={confirm}>
          {busy ? 'Cloning…' : 'Clone and open project'}
        </Button>
      </Dialog.Footer>
    </Dialog.Content>
  </Dialog.Root>
</section>
