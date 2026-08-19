<script lang="ts">
  import { onMount } from 'svelte';
  import {
    ChevronRightIcon,
    File01Icon,
    Folder01Icon,
    FolderOpenIcon
  } from '@hugeicons/core-free-icons';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import StatusOrb from '$lib/components/chrome/StatusOrb.svelte';
  import { clientStore } from '$lib/runtime/client.svelte';
  import type { ClientDirectoryEntry } from '$lib/runtime/contract';
  import { cn } from '$lib/utils';

  let {
    projectName,
    openPath,
    onopen
  }: {
    projectName?: string;
    openPath: string | null;
    onopen: (path: string) => void;
  } = $props();

  let directories = $state<Record<string, ClientDirectoryEntry[]>>({});
  let openFolders = $state<Record<string, boolean>>({ '': true });
  let loadingPaths = $state<Record<string, boolean>>({});
  let errors = $state<Record<string, string>>({});

  async function loadDirectory(path: string) {
    if (directories[path] || loadingPaths[path]) return;
    loadingPaths = { ...loadingPaths, [path]: true };
    const result = await clientStore.listDirectory(path);
    if ('message' in result) {
      errors = { ...errors, [path]: result.message };
    } else {
      directories = { ...directories, [path]: result.entries.items };
      delete errors[path];
      errors = { ...errors };
    }
    delete loadingPaths[path];
    loadingPaths = { ...loadingPaths };
  }

  function toggleFolder(path: string) {
    const nextOpen = !openFolders[path];
    openFolders = { ...openFolders, [path]: nextOpen };
    if (nextOpen) void loadDirectory(path);
  }

  function nodeClasses(active: boolean) {
    return cn(
      'explorer-row',
      active && 'bg-accent/10 text-foreground',
      !active && 'text-muted-foreground'
    );
  }

  function open(entry: ClientDirectoryEntry) {
    if (entry.kind !== 'file' || entry.symlink?.escaping) return;
    onopen(entry.path);
  }

  function contentLabel(entry: ClientDirectoryEntry): string | null {
    if (entry.content.type === 'sniffed' && entry.content.binary) return 'binary';
    if (entry.content.type === 'oversized') return 'large';
    if (entry.content.type === 'unreadable') return 'unreadable';
    return null;
  }

  onMount(() => {
    void loadDirectory('');
  });
</script>

<aside class="flex w-56 shrink-0 flex-col border-l bg-sidebar" data-testid="file-explorer">
  <div class="flex items-center justify-between border-b px-3 py-2">
    <span class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Explorer</span>
    <span class="truncate font-mono text-xs text-muted-foreground" title={projectName ?? undefined}>
      {projectName || 'no workspace'}
    </span>
  </div>

  <div class="min-h-0 flex-1 overflow-auto py-1">
    {#snippet nodeRow(entry: ClientDirectoryEntry, depth: number)}
      <div class="pr-1" style="padding-left: {depth * 12 + 4}px">
        {#if entry.kind === 'directory'}
          <button
            type="button"
            class={cn(nodeClasses(false), 'explorer-row w-full text-left')}
            onclick={() => toggleFolder(entry.path)}
            aria-expanded={Boolean(openFolders[entry.path])}
            data-testid="explorer-folder"
          >
            <HugeiconsIcon
              icon={ChevronRightIcon}
              strokeWidth={2}
              class={cn('size-3.5 shrink-0 transition-transform', openFolders[entry.path] && 'rotate-90')}
            />
            <HugeiconsIcon
              icon={openFolders[entry.path] ? FolderOpenIcon : Folder01Icon}
              strokeWidth={2}
              class="size-3.5 shrink-0"
            />
            <span class="truncate">{entry.name}</span>
          </button>
          {#if openFolders[entry.path]}
            {#if loadingPaths[entry.path]}
              <div class="explorer-row text-muted-foreground/70">loading…</div>
            {:else if errors[entry.path]}
              <div class="explorer-row text-destructive" title={errors[entry.path]}>refused</div>
            {:else if directories[entry.path]?.length === 0}
              <div class="explorer-row text-muted-foreground/70">empty</div>
            {:else}
              {#each directories[entry.path] ?? [] as child (child.path)}
                {@render nodeRow(child, depth + 1)}
              {/each}
            {/if}
          {/if}
        {:else}
          <button
            type="button"
            class={cn(nodeClasses(openPath === entry.path), 'explorer-row w-full text-left')}
            onclick={() => open(entry)}
            disabled={entry.kind !== 'file' || entry.symlink?.escaping === true}
            aria-current={openPath === entry.path ? 'true' : undefined}
            title={entry.symlink?.escaping ? 'smed refused this escaping symlink' : undefined}
            data-testid="explorer-node"
          >
            <span class="w-3.5 shrink-0"></span>
            <HugeiconsIcon icon={File01Icon} strokeWidth={2} class="size-3.5 shrink-0" />
            <span class="truncate">{entry.name}</span>
            {#if entry.ignored}<span class="ml-auto text-[10px]">ignored</span>{/if}
            {#if contentLabel(entry)}<span class="ml-auto text-[10px]">{contentLabel(entry)}</span>{/if}
          </button>
        {/if}
      </div>
    {/snippet}

    {#if loadingPaths['']}
      <div class="px-3 py-2 text-xs text-muted-foreground">Reading project tree…</div>
    {:else if errors['']}
      <div class="px-3 py-2 text-xs text-destructive">{errors['']}</div>
    {:else if directories['']?.length === 0}
      <div class="px-3 py-2 text-xs text-muted-foreground">No entries in this project.</div>
    {:else}
      {#each directories[''] ?? [] as entry (entry.path)}
        {@render nodeRow(entry, 0)}
      {/each}
    {/if}
  </div>

  <div class="shrink-0 border-t px-3 py-2">
    <div class="flex items-center justify-between gap-2">
      <span class="text-xs font-medium text-muted-foreground">Repository</span>
      <StatusOrb state="idle" size={5} />
    </div>
    <p class="mt-1 text-xs text-muted-foreground">
      operator-controlled files · every read is bounded
    </p>
  </div>
</aside>

<style>
  .explorer-row {
    display: flex;
    align-items: center;
    gap: 4px;
    min-height: 26px;
    border-radius: 4px;
    padding-right: 6px;
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
  }
  .explorer-row:hover:not(:disabled) {
    background: color-mix(in oklab, var(--sidebar-foreground) 6%, transparent);
    color: var(--sidebar-foreground);
  }
</style>
