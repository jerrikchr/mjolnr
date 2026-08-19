<script lang="ts">
  import { onDestroy } from 'svelte';
  import { basicSetup } from 'codemirror';
  import { EditorState, type Extension } from '@codemirror/state';
  import { defaultKeymap, indentWithTab, historyKeymap } from '@codemirror/commands';
  import { searchKeymap } from '@codemirror/search';
  import { keymap, EditorView } from '@codemirror/view';
  import { rust } from '@codemirror/lang-rust';
  import { javascript } from '@codemirror/lang-javascript';
  import { json } from '@codemirror/lang-json';
  import { ChevronDownIcon, File01Icon } from '@hugeicons/core-free-icons';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import type { ClientFileOpen } from '$lib/runtime/contract';
  import { cn } from '$lib/utils';

  let {
    path,
    file,
    tabs,
    onselect,
    onclose,
    onsave,
    autosaveEnabled,
    onautosavechange,
    autosaveMessage
  }: {
    path: string;
    file: ClientFileOpen | null;
    tabs: Array<{ path: string; file: ClientFileOpen }>;
    onselect: (path: string) => void;
    onclose: (path: string) => void;
    onsave: (text: string, expectedDigest: string) => Promise<string | null>;
    autosaveEnabled: boolean;
    onautosavechange: (enabled: boolean) => void;
    autosaveMessage: string | null;
  } = $props();

  let host = $state<HTMLDivElement | undefined>(undefined);
  let view: EditorView | null = null;
  const editorStates = new Map<string, EditorState>();
  let activeViewPath = '';
  let loadedDigest = '';
  let saving = $state(false);
  let saveMessage = $state<string | null>(null);
  let autosaveTimer: ReturnType<typeof setTimeout> | undefined;

  function tabId(filePath: string): string {
    return `editor-tab-${filePath.replace(/[^a-zA-Z0-9_-]/g, '-')}`;
  }

  function panelId(filePath: string): string {
    return `editor-panel-${filePath.replace(/[^a-zA-Z0-9_-]/g, '-')}`;
  }

  function languageFor(filePath: string): Extension {
    const lower = filePath.toLowerCase();
    if (lower.endsWith('.rs')) return rust();
    if (lower.endsWith('.json')) return json();
    if (/\.(m?js|c?ts|jsx|tsx)$/.test(lower)) {
      return javascript({ typescript: /\.(ts|tsx)$/.test(lower), jsx: /jsx|tsx$/.test(lower) });
    }
    return [];
  }

  function scheduleAutosave(filePath: string) {
    if (!autosaveEnabled) return;
    if (autosaveTimer) clearTimeout(autosaveTimer);
    autosaveTimer = setTimeout(() => {
      autosaveTimer = undefined;
      if (autosaveEnabled && activeViewPath === filePath) void saveCurrent();
    }, 750);
  }

  function createState(text: string, filePath: string) {
    return EditorState.create({
      doc: text,
      extensions: [
        basicSetup,
        languageFor(filePath),
        keymap.of([
          ...defaultKeymap,
          ...historyKeymap,
          ...searchKeymap,
          indentWithTab,
          {
            key: 'Mod-s',
            run: () => {
              void saveCurrent();
              return true;
            }
          },
          {
            key: 'Mod-w',
            run: () => {
              onclose(filePath);
              return true;
            }
          }
        ]),
        EditorView.updateListener.of((update) => {
          if (!update.docChanged) return;
          saveMessage = null;
          scheduleAutosave(filePath);
        })
      ]
    });
  }

  function activateEditor() {
    if (!host || !file || file.mode.type !== 'editable') return;
    if (view && activeViewPath !== path) {
      editorStates.set(activeViewPath, view.state);
    }
    const state = editorStates.get(path) ?? createState(file.mode.text, path);
    editorStates.set(path, state);
    if (!view) {
      view = new EditorView({ state, parent: host });
    } else if (view.state !== state) {
      view.setState(state);
    }
    activeViewPath = path;
    loadedDigest = file.digest;
    saveMessage = null;
  }

  async function saveCurrent() {
    if (!view || !file || file.mode.type !== 'editable' || saving) return;
    saving = true;
    saveMessage = null;
    saveMessage = await onsave(view.state.doc.toString(), file.digest);
    saving = false;
    if (!saveMessage) loadedDigest = file.digest;
  }

  $effect(() => {
    if (!file || file.mode.type !== 'editable' || !host) {
      if (view && (!file || file.mode.type !== 'editable')) {
        editorStates.set(activeViewPath, view.state);
        view.destroy();
        view = null;
        activeViewPath = '';
      }
      return;
    }
    if (path !== activeViewPath || !view || file.digest !== loadedDigest) activateEditor();
  });

  onDestroy(() => {
    if (autosaveTimer) clearTimeout(autosaveTimer);
    if (view && activeViewPath) editorStates.set(activeViewPath, view.state);
    view?.destroy();
    view = null;
    editorStates.clear();
  });
</script>

<div
  class="flex min-h-0 w-[46%] shrink-0 flex-col border-l bg-background"
  role="tabpanel"
  aria-labelledby={tabId(path)}
  data-testid="editor-pane"
>
  <div
    class="flex min-w-0 shrink-0 items-center gap-1 overflow-x-auto border-b px-2"
    role="tablist"
    aria-label="Open editor files"
  >
    {#each tabs as tab (tab.path)}
      <div class="flex shrink-0 items-center border-r py-1">
        <button
          type="button"
          role="tab"
          class={cn(
            'flex max-w-44 items-center gap-2 rounded-l px-2 py-1.5 text-xs hover:bg-muted',
            tab.path === path && 'bg-accent/10'
          )}
          aria-current={tab.path === path ? 'page' : undefined}
          aria-selected={tab.path === path}
          aria-controls={panelId(tab.path)}
          aria-label={`Open ${tab.path}`}
          id={tabId(tab.path)}
          onclick={() => onselect(tab.path)}
          data-testid={`editor-tab-${tab.path}`}
        >
          <HugeiconsIcon icon={File01Icon} strokeWidth={2} class="size-3.5 shrink-0 text-muted-foreground" />
          <span class="truncate font-mono">{tab.path}</span>
        </button>
        <button
          type="button"
          class="grid size-6 shrink-0 place-items-center rounded-r hover:bg-muted"
          aria-label={`Close ${tab.path}`}
          onclick={() => onclose(tab.path)}
        >
          <HugeiconsIcon icon={ChevronDownIcon} strokeWidth={2} class="size-3.5" />
        </button>
      </div>
    {/each}
    <span class="shrink-0 font-mono text-xs text-muted-foreground">
      {file?.trust === 'operatorControlled' ? 'operator-controlled' : 'unavailable'}
    </span>
    {#if file?.mode.type === 'editable'}
      <label class="ml-auto flex shrink-0 items-center gap-1 font-mono text-xs text-muted-foreground">
        <input
          type="checkbox"
          checked={autosaveEnabled}
          aria-label="Enable autosave"
          data-testid="editor-autosave-toggle"
          onchange={(event) =>
            onautosavechange((event.currentTarget as HTMLInputElement).checked)}
        />
        autosave
      </label>
      <span
        id="editor-status"
        class="shrink-0 font-mono text-xs text-muted-foreground"
        data-testid="editor-status"
      >
        {saving ? 'saving…' : saveMessage ? 'save refused' : '⌘S save · ⌘F find · ⌘W close'}
      </span>
    {/if}
  </div>

  {#if !file}
    <div class="grid min-h-0 flex-1 place-items-center p-6">
      <p class="max-w-xs text-center text-sm text-muted-foreground">No file is open.</p>
    </div>
  {:else if file.mode.type === 'editable'}
    <div
      bind:this={host}
      class="min-h-0 flex-1 overflow-auto text-sm"
      aria-label={`Editor for ${path}`}
      aria-describedby="editor-status"
      aria-keyshortcuts="Control+S Meta+S Control+F Meta+F Control+W Meta+W"
      id={panelId(path)}
      data-testid="code-editor"
    ></div>
    {#if saveMessage}
      <div class="border-t px-3 py-2 text-xs text-destructive" role="alert">{saveMessage}</div>
    {/if}
    {#if autosaveMessage}
      <div class="border-t px-3 py-2 text-xs text-destructive" role="alert" data-testid="editor-autosave-refusal">
        Autosave preference refused: {autosaveMessage}
      </div>
    {/if}
  {:else}
    <div class="min-h-0 flex-1 overflow-auto p-3">
      <div class="mb-2 flex items-center justify-between text-xs text-muted-foreground">
        <span>read-only preview · {file.mode.reason}</span>
        <span>{file.sizeBytes ?? 'unknown'} bytes</span>
      </div>
      <pre class="whitespace-pre-wrap break-words font-mono text-xs leading-5">{file.mode.excerpt}{file.mode.excerptTruncated ? '…' : ''}</pre>
    </div>
  {/if}
</div>
