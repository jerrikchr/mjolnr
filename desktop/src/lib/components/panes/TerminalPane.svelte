<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import {
    Cancel01Icon,
    SquareArrowExpand01Icon,
    SquareArrowShrink01Icon
  } from '@hugeicons/core-free-icons';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import StatusOrb from '$lib/components/chrome/StatusOrb.svelte';
  import type {
    ClientTerminalLayout,
    ClientTerminalSearchResult,
    ClientTerminalSnapshot,
    ClientTerminalSplitDirection
  } from '$lib/runtime/contract';
  import { cn } from '$lib/utils';

  type TauriInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

  let {
    expanded,
    onexpand,
    onclose
  }: {
    expanded: boolean;
    onexpand: (next: boolean) => void;
    onclose: () => void;
  } = $props();

  let terminalHost = $state<HTMLDivElement | undefined>(undefined);
  let terminals = $state<ClientTerminalSnapshot[]>([]);
  let activeTerminalId = $state<string | null>(null);
  let primaryTerminalId = $state<string | null>(null);
  let splitTerminalId = $state<string | null>(null);
  let splitDirection = $state<ClientTerminalSplitDirection | null>(null);
  let terminal = $derived(terminals.find((entry) => entry.id === activeTerminalId) ?? null);
  let primaryTerminal = $derived(terminals.find((entry) => entry.id === primaryTerminalId) ?? null);
  let splitTerminal = $derived(terminals.find((entry) => entry.id === splitTerminalId) ?? null);
  let error = $state<string | null>(null);
  let starting = $state(true);
  let invokeTauri = $state<TauriInvoke | null>(null);
  let cwdInput = $state('');
  let searchQuery = $state('');
  let searchResult = $state<ClientTerminalSearchResult | null>(null);
  let copied = $state(false);
  let pollTimer: ReturnType<typeof setInterval> | undefined;
  let resizeObserver: ResizeObserver | undefined;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let disposed = false;

  function record(snapshot: ClientTerminalSnapshot) {
    const known = terminals.some((entry) => entry.id === snapshot.id);
    terminals = known
      ? terminals.map((entry) => (entry.id === snapshot.id ? snapshot : entry))
      : [...terminals, snapshot];
    if (!activeTerminalId) activeTerminalId = snapshot.id;
    if (!primaryTerminalId) primaryTerminalId = snapshot.id;
  }

  function activate(id: string) {
    if (terminals.some((entry) => entry.id === id)) activeTerminalId = id;
  }

  function statusOrb(status: ClientTerminalSnapshot['status'] | null) {
    if (status === 'running') return 'active';
    if (status === 'failed') return 'attention';
    if (status === 'exited') return 'verified';
    return 'idle';
  }

  function describeError(raw: unknown): string {
    if (typeof raw === 'string') return raw;
    if (raw && typeof raw === 'object' && 'message' in raw) {
      const message = (raw as { message?: unknown }).message;
      if (typeof message === 'string') return message;
    }
    return String(raw);
  }

  async function refresh() {
    if (!invokeTauri || !terminal) return;
    try {
      record(await invokeTauri<ClientTerminalSnapshot>('terminal_snapshot', { id: terminal.id }));
    } catch (raw: unknown) {
      error = describeError(raw);
    }
  }

  async function send(data: string) {
    if (!invokeTauri || !terminal || terminal.status !== 'running' || !data) return;
    try {
      await invokeTauri('terminal_input', { input: { id: terminal.id, data } });
      await refresh();
    } catch (raw: unknown) {
      error = describeError(raw);
    }
  }

  function keyData(event: KeyboardEvent): string | null {
    if (event.ctrlKey && event.key.length === 1) {
      const code = event.key.toLowerCase().charCodeAt(0);
      if (code >= 97 && code <= 122) return String.fromCharCode(code - 96);
    }
    const sequences: Record<string, string> = {
      Enter: '\r',
      Backspace: '\x7f',
      Tab: '\t',
      Escape: '\x1b',
      ArrowUp: '\x1b[A',
      ArrowDown: '\x1b[B',
      ArrowRight: '\x1b[C',
      ArrowLeft: '\x1b[D',
      Home: '\x1b[H',
      End: '\x1b[F',
      Delete: '\x1b[3~',
      PageUp: '\x1b[5~',
      PageDown: '\x1b[6~'
    };
    if (sequences[event.key]) return sequences[event.key];
    return !event.metaKey && !event.altKey && event.key.length === 1 ? event.key : null;
  }

  function handleKeydown(event: KeyboardEvent) {
    const data = keyData(event);
    if (!data) return;
    event.preventDefault();
    void send(data);
  }

  function dimensions(): { rows: number; cols: number } {
    const width = terminalHost?.clientWidth ?? 0;
    const height = terminalHost?.clientHeight ?? 0;
    return {
      rows: Math.max(1, Math.min(200, Math.floor(height / 19.2))),
      cols: Math.max(1, Math.min(400, Math.floor(width / 7.2)))
    };
  }

  async function resize() {
    if (!invokeTauri || !terminal || terminal.status !== 'running') return;
    const next = dimensions();
    if (next.rows === terminal.rows && next.cols === terminal.cols) return;
    try {
      await invokeTauri('terminal_resize', { resize: { id: terminal.id, ...next } });
      record({ ...terminal, ...next });
    } catch (raw: unknown) {
      error = describeError(raw);
    }
  }

  async function saveLayout() {
    if (!invokeTauri || !primaryTerminal) return;
    const layout: ClientTerminalLayout = {
      primaryCwd: primaryTerminal.cwd,
      splitDirection: splitDirection ?? undefined,
      secondaryCwd: splitTerminal?.cwd
    };
    try {
      await invokeTauri('terminal_layout_save', { layout });
    } catch (raw: unknown) {
      error = describeError(raw);
    }
  }

  async function start() {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) {
      error = 'Terminal unavailable outside the Tauri runtime';
      starting = false;
      return;
    }
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      invokeTauri = invoke as TauriInvoke;
      let layout: ClientTerminalLayout = { primaryCwd: '' };
      try {
        layout = await invokeTauri<ClientTerminalLayout>('terminal_layout_load');
      } catch (raw: unknown) {
        error = describeError(raw);
      }
      cwdInput = layout.primaryCwd;
      await startTerminal(layout.primaryCwd);
      if (layout.splitDirection && layout.secondaryCwd !== undefined) {
        const secondary = await startTerminal(layout.secondaryCwd);
        if (secondary && primaryTerminalId && secondary.id !== primaryTerminalId) {
          splitTerminalId = secondary.id;
          splitDirection = layout.splitDirection;
        }
      }
      pollTimer = setInterval(() => void refresh(), 100);
      await resize();
      terminalHost?.focus();
    } catch (raw: unknown) {
      error = describeError(raw);
    } finally {
      starting = false;
    }
  }

  async function startTerminal(cwd?: string): Promise<ClientTerminalSnapshot | null> {
    if (!invokeTauri) return null;
    try {
      const started = await invokeTauri<ClientTerminalSnapshot>('terminal_start', {
        rows: 24,
        cols: 100,
        cwd: cwd?.trim() || undefined
      });
      if (disposed) {
        await invokeTauri('terminal_stop', { id: started.id });
        return null;
      }
      record(started);
      activeTerminalId = started.id;
      await resize();
      return started;
    } catch (raw: unknown) {
      error = describeError(raw);
      return null;
    }
  }

  function stopTerminal(id: string) {
    if (!invokeTauri) return;
    void invokeTauri<ClientTerminalSnapshot>('terminal_stop', { id })
      .then(record)
      .catch((raw: unknown) => (error = describeError(raw)));
  }

  async function closeTerminal(id: string) {
    if (!invokeTauri) return;
    try {
      await invokeTauri('terminal_close', { id });
      terminals = terminals.filter((entry) => entry.id !== id);
      if (activeTerminalId === id) {
        activeTerminalId = terminals[0]?.id ?? null;
      }
      if (primaryTerminalId === id) {
        primaryTerminalId = terminals[0]?.id ?? null;
      }
      if (splitTerminalId === id) {
        splitTerminalId = null;
        splitDirection = null;
      }
      await saveLayout();
    } catch (raw: unknown) {
      error = describeError(raw);
    }
  }

  async function restartTerminal(id: string) {
    if (!invokeTauri) return;
    try {
      const cwd = terminals.find((entry) => entry.id === id)?.cwd;
      const wasPrimary = primaryTerminalId === id;
      const wasSplit = splitTerminalId === id;
      await invokeTauri('terminal_close', { id });
      terminals = terminals.filter((entry) => entry.id !== id);
      const restarted = await startTerminal(cwd);
      if (restarted && wasPrimary) primaryTerminalId = restarted.id;
      if (restarted && wasSplit) splitTerminalId = restarted.id;
      await saveLayout();
    } catch (raw: unknown) {
      error = describeError(raw);
    }
  }

  function stop() {
    if (!invokeTauri) return;
    for (const session of terminals) {
      if (session.status === 'running') stopTerminal(session.id);
    }
  }

  async function createSplit(direction: ClientTerminalSplitDirection) {
    if (!invokeTauri || !primaryTerminal) return;
    if (splitTerminalId && splitTerminal) {
      splitDirection = splitDirection === direction ? null : direction;
      if (!splitDirection) splitTerminalId = null;
      await saveLayout();
      return;
    }
    const secondary = await startTerminal(cwdInput.trim() || primaryTerminal.cwd);
    if (!secondary || secondary.id === primaryTerminal.id) return;
    splitTerminalId = secondary.id;
    splitDirection = direction;
    await saveLayout();
  }

  async function handleWheel(event: WheelEvent) {
    if (!invokeTauri || !terminal || event.deltaY === 0) return;
    event.preventDefault();
    try {
      record(
        await invokeTauri<ClientTerminalSnapshot>('terminal_scroll', {
          scroll: { id: terminal.id, rows: event.deltaY > 0 ? 3 : -3 }
        })
      );
    } catch (raw: unknown) {
      error = describeError(raw);
    }
  }

  async function copyScreen() {
    if (!terminal) return;
    try {
      await navigator.clipboard.writeText(terminal.screen);
      copied = true;
      setTimeout(() => (copied = false), 1200);
    } catch (raw: unknown) {
      error = describeError(raw);
    }
  }

  $effect(() => {
    clearTimeout(searchTimer);
    searchResult = null;
    const query = searchQuery.trim();
    const id = terminal?.id;
    const invoke = invokeTauri;
    if (!invoke || !id || !query) return;
    searchTimer = setTimeout(() => {
      void invoke<ClientTerminalSearchResult>('terminal_search', {
        search: { id, query }
      })
        .then((result) => (searchResult = result))
        .catch((raw: unknown) => (error = describeError(raw)));
    }, 150);
    return () => clearTimeout(searchTimer);
  });

  onMount(() => {
    void start();
    resizeObserver = new ResizeObserver(() => void resize());
    if (terminalHost) resizeObserver.observe(terminalHost);
  });

  onDestroy(() => {
    disposed = true;
    if (pollTimer) clearInterval(pollTimer);
    resizeObserver?.disconnect();
    stop();
  });
</script>

<div
  class="flex shrink-0 flex-col border-t bg-background"
  style="height: {expanded ? 420 : 240}px"
  data-testid="terminal-pane"
>
  <div class="flex min-w-0 shrink-0 items-center gap-2 border-b px-2">
    <div class="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
      {#each terminals as session, index (session.id)}
        <div class="flex shrink-0 items-center border-b-2 {session.id === activeTerminalId ? 'border-accent-bright' : 'border-transparent'}">
          <button
            type="button"
            class="flex items-center gap-1.5 rounded px-2.5 py-1.5 text-xs font-medium hover:bg-muted"
            aria-label={`Switch to operator shell ${index + 1}`}
            aria-current={session.id === activeTerminalId ? 'page' : undefined}
            onclick={() => activate(session.id)}
            data-testid={`terminal-tab-${index + 1}`}
          >
            <StatusOrb state={statusOrb(session.status)} size={5} />
            <span class="font-mono">shell {index + 1}</span>
          </button>
          <button
            type="button"
            class="grid size-5 place-items-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
            aria-label={`Stop operator shell ${index + 1}`}
            disabled={session.status !== 'running'}
            onclick={() => stopTerminal(session.id)}
          >
            {session.status === 'running' ? '×' : '·'}
          </button>
          {#if session.status === 'exited' || session.status === 'failed'}
            <button
              type="button"
              class="grid size-5 place-items-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
              aria-label={`Restart operator shell ${index + 1}`}
              onclick={() => void restartTerminal(session.id)}
            >
              ↻
            </button>
            <button
              type="button"
              class="grid size-5 place-items-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
              aria-label={`Close operator shell ${index + 1}`}
              onclick={() => void closeTerminal(session.id)}
            >
              ×
            </button>
          {/if}
        </div>
      {/each}
      <button
        type="button"
        class="grid size-7 shrink-0 place-items-center rounded text-sm text-muted-foreground hover:bg-muted hover:text-foreground"
        aria-label="New operator shell"
        disabled={starting || !invokeTauri}
        onclick={() => void startTerminal(cwdInput)}
      >
        +
      </button>
      <input
        class="h-7 w-28 rounded border bg-background px-2 font-mono text-xs"
        bind:value={cwdInput}
        aria-label="Terminal working directory"
        placeholder="cwd (relative)"
      />
      <button
        type="button"
        class="rounded px-2 py-1 text-xs hover:bg-muted"
        aria-label="Split terminal horizontally"
        onclick={() => void createSplit('horizontal')}
        disabled={starting || !invokeTauri}
      >H</button>
      <button
        type="button"
        class="rounded px-2 py-1 text-xs hover:bg-muted"
        aria-label="Split terminal vertically"
        onclick={() => void createSplit('vertical')}
        disabled={starting || !invokeTauri}
      >V</button>
    </div>
    <span class="shrink-0 font-mono text-xs text-muted-foreground">
      {starting ? 'starting…' : terminal ? `pty · ${terminal.status}` : 'operator-controlled'}
    </span>
    {#if terminal?.status === 'running'}<span class="shrink-0 text-[10px] text-muted-foreground">click terminal to type</span>{/if}
    <div class="ml-auto flex shrink-0 items-center gap-0.5">
      <input
        class="h-7 w-32 rounded border bg-background px-2 font-mono text-xs"
        bind:value={searchQuery}
        aria-label="Search terminal scrollback"
        placeholder="search scrollback"
      />
      <button
        type="button"
        class="rounded px-2 py-1 text-xs hover:bg-muted"
        aria-label="Copy terminal screen"
        onclick={() => void copyScreen()}
        disabled={!terminal}
      >{copied ? 'Copied' : 'Copy'}</button>
      <button
        type="button"
        class="grid size-7 place-items-center rounded hover:bg-muted"
        aria-label={expanded ? 'Collapse terminal' : 'Expand terminal'}
        onclick={() => onexpand(!expanded)}
        data-testid="terminal-expand"
      >
        <HugeiconsIcon
          icon={expanded ? SquareArrowShrink01Icon : SquareArrowExpand01Icon}
          strokeWidth={2}
          class="size-4"
        />
      </button>
      <button
        type="button"
        class="grid size-7 place-items-center rounded hover:bg-muted"
        aria-label="Close terminal (⌘\\)"
        onclick={() => {
          stop();
          onclose();
        }}
      >
        <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} class="size-4" />
      </button>
    </div>
  </div>

  {#if splitDirection && primaryTerminal && splitTerminal}
    <div class={cn('min-h-0 flex-1', splitDirection === 'horizontal' ? 'grid grid-rows-2' : 'grid grid-cols-2')}>
      <div
        class="term-body min-h-0 overflow-auto whitespace-pre-wrap border-r px-3 py-2 outline-none"
        tabindex="0"
        role="textbox"
        aria-label="Primary operator terminal"
        aria-live="polite"
        onclick={() => activate(primaryTerminal.id)}
        onkeydown={handleKeydown}
        onwheel={handleWheel}
      >{primaryTerminal.screen}</div>
      <div
        class="term-body min-h-0 overflow-auto whitespace-pre-wrap px-3 py-2 outline-none"
        tabindex="0"
        role="textbox"
        aria-label="Split operator terminal"
        aria-live="polite"
        onclick={() => activate(splitTerminal.id)}
        onkeydown={handleKeydown}
        onwheel={handleWheel}
      >{splitTerminal.screen}</div>
    </div>
  {:else}
    <div
      bind:this={terminalHost}
      class={cn(
        'term-body min-h-0 flex-1 overflow-auto whitespace-pre-wrap px-3 py-2 outline-none',
        !terminal && 'text-muted-foreground'
      )}
      tabindex="0"
      role="textbox"
      aria-label="Operator terminal"
      aria-live="polite"
      onclick={() => terminalHost?.focus()}
      onkeydown={handleKeydown}
      onwheel={handleWheel}
    >{terminal?.screen ?? (error ?? 'Connecting to the Rust-owned terminal…')}</div>
  {/if}
  {#if searchQuery && searchResult}
    <div class="border-t px-3 py-1.5 text-xs text-muted-foreground" role="status" data-testid="terminal-search-result">
      {searchResult.matches.length} bounded scrollback match{searchResult.matches.length === 1 ? '' : 'es'}{searchResult.truncated ? ' (more omitted)' : ''}
      {#if searchResult.matches[0]}
        <button
          type="button"
          class="ml-2 underline"
          onclick={() => {
            if (!invokeTauri || !terminal || !searchResult?.matches[0]) return;
            void invokeTauri<ClientTerminalSnapshot>('terminal_scroll', {
              scroll: {
                id: terminal.id,
                rows: searchResult.matches[0].scrollbackOffset - terminal.scrollbackOffset
              }
            }).then(record).catch((raw: unknown) => (error = describeError(raw)));
          }}
        >Jump to first</button>
      {/if}
    </div>
  {/if}
  {#if error && terminal}
    <div class="border-t px-3 py-1.5 text-xs text-destructive" role="alert">{error}</div>
  {/if}
</div>

<style>
  .term-body {
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.6;
  }
</style>
