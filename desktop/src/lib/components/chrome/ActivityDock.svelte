<!--
  ActivityDock: Far-left 48px global dock for workspace navigation,
  surface switching (Chat, Plan, Board, Graph, Changes, Verify, Attention),
  and bottom settings/status anchors (SOUL.md, Council, Provider Auth, Health).
-->
<script lang="ts">
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import {
    Message01Icon,
    Task01Icon,
    DashboardSquare01Icon,
    FileEditIcon,
    CheckmarkCircle02Icon,
    Notification02Icon,
    SearchIcon,
    BotIcon,
    MaskTheater01Icon,
    SparklesIcon
  } from '@hugeicons/core-free-icons';
  import AppEmblem from '$lib/components/chrome/AppEmblem.svelte';
  import StatusOrb from '$lib/components/chrome/StatusOrb.svelte';
  import { cn } from '$lib/utils';
  import { clientStore } from '$lib/runtime/client.svelte';

  type SurfaceId = 'Conversation' | 'Plan' | 'Board' | 'Changes' | 'Verify' | 'Attention';

  let {
    activeSurface = $bindable('Conversation'),
    attentionCount = 0,
    onopenproviderauth,
    onopengovernance,
    onopengraph
  }: {
    activeSurface: SurfaceId;
    attentionCount?: number;
    onopenproviderauth?: () => void;
    onopengovernance?: (tab: string) => void;
    onopengraph?: () => void;
  } = $props();

  let snap = $derived(clientStore.snapshot);
  let isConnected = $derived(clientStore.connected);

  const dockSurfaces: Array<{
    id: SurfaceId;
    label: string;
    shortcut: string;
    icon: typeof Message01Icon;
  }> = [
    { id: 'Conversation', label: 'Chat', shortcut: '⌘1', icon: Message01Icon },
    { id: 'Plan', label: 'Plan & Tasks', shortcut: '⌘2', icon: Task01Icon },
    { id: 'Board', label: 'Board', shortcut: '⌘3', icon: DashboardSquare01Icon },
    { id: 'Changes', label: 'Changes & Git', shortcut: '⌘4', icon: FileEditIcon },
    { id: 'Verify', label: 'Verify', shortcut: '⌘5', icon: CheckmarkCircle02Icon },
    { id: 'Attention', label: 'Attention', shortcut: '⌘6', icon: Notification02Icon }
  ];
</script>

<aside
  class="flex flex-col items-center justify-between w-12 shrink-0 border-r border-sidebar-border/60 bg-[#090a0d] dark:bg-[#07080a] py-2.5 z-20 select-none shadow-[1px_0_0_rgba(0,0,0,0.2)]"
  aria-label="Activity Dock"
>
  <!-- Top: Logo & Surfaces -->
  <div class="flex flex-col items-center gap-2.5 w-full">
    <!-- Brand Mark Anchor -->
    <button
      type="button"
      class="p-1 rounded-lg hover:bg-sidebar-accent/80 transition-transform active:scale-95 cursor-pointer"
      onclick={() => (activeSurface = 'Conversation')}
      title="mjolnr — The Hammer of Code"
    >
      <AppEmblem size={24} />
    </button>

    <div class="w-6 h-px bg-border/50 my-0.5"></div>

    <!-- Surfaces Icons -->
    <nav class="flex flex-col items-center gap-1 w-full px-1.5" aria-label="Main Surfaces">
      {#each dockSurfaces as item (item.id)}
        {@const isActive = activeSurface === item.id}
        <button
          type="button"
          class={cn(
            'relative flex items-center justify-center size-9 rounded-lg transition-all cursor-pointer group',
            isActive
              ? 'bg-primary/15 text-primary shadow-[inset_0_0_0_1px_rgba(16,185,129,0.3)]'
              : 'text-muted-foreground hover:text-foreground hover:bg-sidebar-accent/70'
          )}
          onclick={() => (activeSurface = item.id)}
          title={`${item.label} (${item.shortcut})`}
          aria-label={item.label}
        >
          <!-- Active Pill Indicator on Left Edge -->
          {#if isActive}
            <span class="absolute -left-1.5 top-1.5 bottom-1.5 w-1 rounded-r bg-primary shadow-[0_0_6px_var(--accent-glow)]"></span>
          {/if}

          <HugeiconsIcon icon={item.icon} strokeWidth={isActive ? 2.2 : 1.8} class="size-4.5" />

          <!-- Attention Counter Badge -->
          {#if item.id === 'Attention' && attentionCount > 0}
            <span class="absolute -top-0.5 -right-0.5 flex size-4 items-center justify-center rounded-full bg-destructive text-[9px] font-bold text-destructive-foreground">
              {attentionCount}
            </span>
          {/if}
        </button>
      {/each}

      <!-- Code Graph Trigger -->
      <button
        type="button"
        class="flex items-center justify-center size-9 rounded-lg text-muted-foreground hover:text-foreground hover:bg-sidebar-accent/70 transition-all cursor-pointer"
        onclick={onopengraph}
        title="Knowledge & Code Graph"
        aria-label="Code Graph"
      >
        <HugeiconsIcon icon={SearchIcon} strokeWidth={1.8} class="size-4.5" />
      </button>
    </nav>
  </div>

  <!-- Bottom: Council, Personas, Provider Settings & Runtime Status -->
  <div class="flex flex-col items-center gap-1 w-full px-1.5 pt-2 border-t border-sidebar-border/40">
    <!-- Multi-Agent Council -->
    <button
      type="button"
      class="flex items-center justify-center size-9 rounded-lg text-muted-foreground hover:text-primary hover:bg-primary/10 transition-all cursor-pointer"
      onclick={() => onopengovernance?.('council')}
      title="Council & Fleet Review (⌘G)"
      aria-label="Council"
    >
      <HugeiconsIcon icon={BotIcon} strokeWidth={1.8} class="size-4.5" />
    </button>

    <!-- SOUL.md / Persona -->
    <button
      type="button"
      class="flex items-center justify-center size-9 rounded-lg text-muted-foreground hover:text-primary hover:bg-primary/10 transition-all cursor-pointer"
      onclick={() => onopengovernance?.('soul')}
      title={`Active Persona: ${snap.activePersona ?? 'Route default'} (SOUL.md)`}
      aria-label="Personas and SOUL"
    >
      <HugeiconsIcon icon={MaskTheater01Icon} strokeWidth={1.8} class="size-4.5" />
    </button>

    <!-- Provider Connection & Settings -->
    <button
      type="button"
      class="flex items-center justify-center size-9 rounded-lg text-muted-foreground hover:text-foreground hover:bg-sidebar-accent/80 transition-all cursor-pointer"
      onclick={onopenproviderauth}
      title="Model Providers & API Keys"
      aria-label="Providers & Keys"
    >
      <HugeiconsIcon icon={SparklesIcon} strokeWidth={1.8} class="size-4.5 text-primary" />
    </button>

    <!-- Connection Status Indicator -->
    <div class="pt-1.5 pb-0.5 flex items-center justify-center" title={isConnected ? 'Runtime Connected & Active' : 'Runtime Disconnected'}>
      <StatusOrb state={snap.runActive ? 'active' : isConnected ? 'verified' : 'idle'} size={6} />
    </div>
  </div>
</aside>
