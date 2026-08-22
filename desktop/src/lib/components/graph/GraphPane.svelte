<script lang="ts">
  import { onMount } from 'svelte';
  import { GitGraphIcon, HelpCircleIcon, Maximize01Icon, Minimize01Icon } from '@hugeicons/core-free-icons';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import { Background, Controls, MiniMap, SvelteFlow, ViewportPortal, type Edge, type Node } from '@xyflow/svelte';
  import { forceCenter, forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type SimulationLinkDatum, type SimulationNodeDatum } from 'd3-force';
  import '@xyflow/svelte/dist/style.css';
  import { clientStore, type ClientRefusal } from '$lib/runtime/client.svelte';
  import type { ClientGraphDirection, ClientGraphNode, ClientGraphPage, ClientGraphStatus } from '$lib/runtime/contract';
  import GraphNode from '$lib/components/graph/GraphNode.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import * as Empty from '$lib/components/ui/empty';
  import { Input } from '$lib/components/ui/input';

  type GraphNodeData = {
    path: string;
    label: string;
    language: string;
    symbols: number;
    degree: number;
    accent: string;
    inCycle: boolean;
    isArticulationPoint: boolean;
  };
  type FlowNode = Node<GraphNodeData, 'graph'>;
  type LayoutNode = SimulationNodeDatum & { id: string; clusterIndex: number };
  type Realm = { id: number; label: string; x: number; y: number; radius: number; accent: string };

  let {
    onopen,
    expanded = false,
    onexpand
  }: {
    onopen?: (path: string) => void;
    expanded?: boolean;
    onexpand?: (expanded: boolean) => void;
  } = $props();

  const nodeTypes = { graph: GraphNode };
  const palette = ['#19d3ae', '#8b7cff', '#ffb454', '#55a8ff', '#ef76c8', '#9fe870', '#fb7185', '#38bdf8'];
  let page = $state<ClientGraphPage | null>(null);
  let error = $state<ClientRefusal | null>(null);
  let loading = $state(false);
  let direction = $state<ClientGraphDirection>('both');
  let focusPath = $state<string | null>(null);
  let search = $state('');
  let flowNodes = $state<FlowNode[]>([]);
  let flowEdges = $state<Edge[]>([]);
  let realms = $state<Realm[]>([]);
  let graphStatus = $state<ClientGraphStatus | null>(null);
  let guideOpen = $state(true);
  let metaMode = $state(false);
  let statusTimer: ReturnType<typeof setInterval> | null = null;
  let selected = $derived(page?.nodes.find((node) => node.path === focusPath) ?? page?.nodes[0]);

  function hash(value: string) {
    let result = 0;
    for (let index = 0; index < value.length; index += 1) result = (result * 31 + value.charCodeAt(index)) | 0;
    return Math.abs(result);
  }

  function label(path: string) {
    const parts = path.split('/');
    return parts.at(-1) ?? path;
  }

  function layout(nodes: ClientGraphNode[], edges: ClientGraphPage['edges']) {
    const communities = [...new Set(nodes.map((node) => node.community ?? -1))].sort((a, b) => a - b);
    const clusterCenters = [
      { x: -470, y: -270 }, { x: 470, y: -270 }, { x: -540, y: 120 },
      { x: 540, y: 120 }, { x: -180, y: 350 }, { x: 180, y: 350 },
      { x: 0, y: -60 }, { x: 0, y: 180 }
    ];
    const communityIndex = new Map(communities.map((id, index) => [id, index]));
    realms = communities.map((id, index) => {
      const center = clusterCenters[index % clusterCenters.length];
      const count = nodes.filter((node) => (node.community ?? -1) === id).length;
      return { id, label: id < 0 ? 'Unresolved realm' : `Realm ${id + 1}`, x: center.x, y: center.y, radius: 120 + Math.min(100, count * 8), accent: palette[index % palette.length] };
    });
    const simulationNodes: LayoutNode[] = nodes.map((node) => {
      const clusterIndex = (communityIndex.get(node.community ?? -1) ?? 0) % clusterCenters.length;
      const center = clusterCenters[clusterIndex];
      return { id: node.path, x: center.x + ((hash(node.path) % 180) - 90), y: center.y + ((hash(`${node.path}:y`) % 130) - 65), clusterIndex };
    });
    const simulationLinks: SimulationLinkDatum<LayoutNode>[] = edges.map((edge) => ({ source: edge.from, target: edge.to }));
    if (metaMode) {
      const communityByPath = new Map(nodes.map((node) => [node.path, node.community ?? -1]));
      const realmDegrees = new Map(realms.map((realm) => [realm.id, 0]));
      const realmEdges = new Set<string>();
      for (const edge of edges) {
        const from = communityByPath.get(edge.from) ?? -1;
        const to = communityByPath.get(edge.to) ?? -1;
        if (from === to) continue;
        const key = from < to ? `${from}:${to}` : `${to}:${from}`;
        realmEdges.add(key);
        realmDegrees.set(from, (realmDegrees.get(from) ?? 0) + 1);
        realmDegrees.set(to, (realmDegrees.get(to) ?? 0) + 1);
      }
      flowNodes = realms.map((realm) => ({
        id: `realm-${realm.id}`,
        type: 'graph',
        position: { x: realm.x, y: realm.y },
        data: { path: realm.label, label: realm.label, language: 'realm', symbols: nodes.filter((node) => (node.community ?? -1) === realm.id).length, degree: realmDegrees.get(realm.id) ?? 0, accent: realm.accent, inCycle: false, isArticulationPoint: false },
        draggable: true,
        selectable: true,
        focusable: true,
        ariaLabel: `${realm.label} community`
      }));
      flowEdges = [...realmEdges].map((key, index) => {
        const [from, to] = key.split(':');
        return { id: `realm-edge-${index}`, source: `realm-${from}`, target: `realm-${to}`, type: 'default', markerEnd: { type: 'arrowclosed' }, style: 'stroke: rgba(143,151,190,.5); stroke-width: 2;' };
      });
      return;
    }
    const simulation = forceSimulation<LayoutNode>(simulationNodes)
      .randomSource(() => 0.5)
      .force('link', forceLink<LayoutNode, SimulationLinkDatum<LayoutNode>>(simulationLinks).id((node) => node.id).distance(150).strength(0.34))
      .force('charge', forceManyBody().strength(-230))
      .force('collide', forceCollide(92))
      .force('center', forceCenter(0, 40).strength(0.12))
      .force('x', forceX<LayoutNode>((node) => clusterCenters[node.clusterIndex].x).strength(0.12))
      .force('y', forceY<LayoutNode>((node) => clusterCenters[node.clusterIndex].y).strength(0.12))
      .stop();
    for (let tick = 0; tick < 180; tick += 1) simulation.tick();

    flowNodes = nodes.map((node, index) => {
      const point = simulationNodes[index];
      const accent = palette[(communityIndex.get(node.community ?? -1) ?? hash(node.language)) % palette.length];
      return {
        id: node.path,
        type: 'graph',
        position: { x: point.x ?? 0, y: point.y ?? 0 },
        data: { path: node.path, label: label(node.path), language: node.language, symbols: node.symbols.length, degree: node.degree, accent, inCycle: node.inCycle, isArticulationPoint: node.isArticulationPoint },
        draggable: true,
        selectable: true,
        focusable: true,
        ariaLabel: `Code graph node ${node.path}`
      };
    });
    flowEdges = edges.map((edge, index) => ({
      id: `${edge.from}->${edge.to}-${index}`,
      source: edge.from,
      target: edge.to,
      type: 'default',
      animated: false,
      markerEnd: { type: 'arrowclosed' },
      style: `stroke: ${edge.from === focusPath || edge.to === focusPath ? 'rgba(25,211,174,.95)' : edge.confidenceBps < 1000 ? 'rgba(251,180,84,.38)' : 'rgba(143,151,190,.35)'}; stroke-width: ${edge.from === focusPath || edge.to === focusPath ? 2.6 : 1.15}; ${edge.confidenceBps < 1000 ? 'stroke-dasharray: 5 5;' : ''}`
    }));
  }

  async function load(path: string | null = focusPath) {
    loading = true;
    error = null;
    startStatusPolling();
    const trimmed = search.trim();
    const result = await clientStore.queryGraph({ path, depth: 2, direction, search: trimmed ? trimmed : null });
    if ('message' in result) { error = result; page = null; flowNodes = []; flowEdges = []; realms = []; }
    else { page = result; layout(result.nodes, result.edges); }
    loading = false;
    stopStatusPolling();
    const status = await clientStore.queryGraphStatus();
    if (!('message' in status)) graphStatus = status;
  }

  function startStatusPolling() {
    if (statusTimer) return;
    statusTimer = setInterval(async () => {
      const status = await clientStore.queryGraphStatus();
      if (!('message' in status)) graphStatus = status;
    }, 250);
  }

  function stopStatusPolling() {
    if (statusTimer) { clearInterval(statusTimer); statusTimer = null; }
  }

  function toggleMetaMode() {
    metaMode = !metaMode;
    if (page) layout(page.nodes, page.edges);
  }

  function focus(path: string) { focusPath = path; void load(path); }
  function reset() { focusPath = null; void load(null); }
  onMount(() => { void load(null); return stopStatusPolling; });
</script>

<section class="flex min-h-0 h-full flex-col bg-background" data-testid="graph-pane">
  <header class="flex shrink-0 flex-wrap items-center gap-2 border-b px-4 py-3">
    <div class="mr-auto flex min-w-0 items-center gap-2">
      <span class="flex size-7 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"><HugeiconsIcon icon={GitGraphIcon} strokeWidth={2} class="size-4" /></span>
      <div class="min-w-0"><h2 class="font-medium">Yggdrasil</h2><p class="text-xs text-muted-foreground">Code graph · spatial source relationships</p></div>
    </div>
    <Input class="h-8 w-44" placeholder="Search path or symbol" aria-label="Graph search" bind:value={search} onkeydown={(event: KeyboardEvent) => { if (event.key === 'Enter') void load(focusPath); if (event.key === 'Escape') { search = ''; void load(focusPath); } }} />
    <Button variant="ghost" size="sm" onclick={() => void load(focusPath)}>Search</Button>
    <select class="h-8 rounded-md border bg-background px-2 text-xs" aria-label="Graph direction" bind:value={direction} onchange={() => void load(focusPath)}>
      <option value="both">imports + importers</option><option value="imports">imports</option><option value="importers">importers</option>
    </select>
    <Button variant="ghost" size="sm" onclick={reset} disabled={!focusPath}>Reset</Button>
    <Button variant={metaMode ? 'default' : 'ghost'} size="sm" aria-label="Toggle realm overview" onclick={toggleMetaMode}>{metaMode ? 'File leaves' : 'Realm overview'}</Button>
    <Button variant="ghost" size="icon-sm" aria-label={expanded ? 'Exit full-width graph' : 'Open graph full width'} title={expanded ? 'Exit full-width graph' : 'Open graph full width'} onclick={() => onexpand?.(!expanded)}>
      <HugeiconsIcon icon={expanded ? Minimize01Icon : Maximize01Icon} strokeWidth={2} />
    </Button>
  </header>

  <div class="min-h-0 flex-1 overflow-hidden p-3">
    {#if loading}<div class="mapping-state"><span class="mapping-pulse"></span><div><p class="text-sm font-medium">{graphStatus?.detail ?? 'Starting the graph mapper…'}{#if graphStatus?.filesTotal} <span class="font-mono text-primary">({graphStatus.filesScanned}/{graphStatus.filesTotal})</span>{/if}</p><p class="text-xs text-muted-foreground">The source is being read locally; this surface remains read-only until the mapping is ready.</p></div></div>
    {:else if error}<div class="m-3 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm" role="alert">{#if error.code}<span class="font-mono text-xs">{error.code} · </span>{/if}{error.message}</div>
    {:else if page && page.nodes.length === 0}<Empty.Root><Empty.Header><Empty.Title>No graph nodes</Empty.Title><Empty.Description>{search.trim() ? `No nodes match "${search.trim()}".` : 'No supported source files were observed in this workspace.'}</Empty.Description></Empty.Header></Empty.Root>
    {:else if page}
      <div class="flex h-full min-h-0 flex-col gap-2">
        <div class="flex flex-wrap gap-1.5 px-1 text-xs"><Badge variant="secondary">{page.nodes.length} leaves</Badge><Badge variant="outline">{page.edges.length} threads</Badge><Badge variant="outline">{page.summary.communities} realms</Badge>{#if page.summary.cycleNodes > 0}<Badge variant="outline">{page.summary.cycleNodes} cycle-bound</Badge>{/if}{#if page.truncated}<Badge variant="outline">bounded</Badge>{/if}<span class="ml-auto self-center text-muted-foreground">Drag leaves · scroll to zoom · space + drag to pan</span></div>
        {#if page.summary.unsupportedLanguages.length > 0}<div class="coverage-note"><span class="coverage-dot"></span><span><b>Partial map:</b> {page.summary.unsupportedLanguages.join(', ')} source files are present but not parsed yet. The leaves shown are not the whole project.</span></div>{/if}
        <details class="coverage-details"><summary>Language coverage</summary><div class="coverage-grid"><div class="coverage-row coverage-heading"><span>Language</span><span>Files</span><span>Extraction</span><span>Resolver</span><span>Calls</span></div>{#each page.summary.languages as capability (capability.language)}<div class="coverage-row"><span class="font-mono">{capability.language}</span><span>{capability.files}</span><span>{capability.extraction}</span><span>{capability.resolver}</span><span>{capability.callGraph ? 'available' : 'not mapped'}</span></div>{/each}</div></details>
        {#if guideOpen}<div class="guide-card"><div class="guide-icon"><HugeiconsIcon icon={GitGraphIcon} strokeWidth={2} /></div><div class="min-w-0 flex-1"><p class="text-sm font-semibold">How to read Yggdrasil</p><p class="mt-1 text-xs text-muted-foreground">Each leaf is a source file. Its color groups it with a realm; its number is how many relationships it has. Threads are parsed imports. The trunk and wells are orientation landmarks, not extra data.</p><div class="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-muted-foreground"><span><b class="text-foreground">Click</b> a leaf to inspect it</span><span><b class="text-foreground">Drag</b> to arrange your view</span><span><b class="text-foreground">Scroll</b> to zoom</span><span><b class="text-foreground">Space + drag</b> to pan</span></div></div><button type="button" class="guide-close" aria-label="Hide graph guide" onclick={() => (guideOpen = false)}>×</button></div>{:else}<button type="button" class="guide-reopen" onclick={() => (guideOpen = true)}><HugeiconsIcon icon={HelpCircleIcon} strokeWidth={2} />How to read this graph</button>{/if}
        <div class="relative min-h-0 flex-1 overflow-hidden rounded-xl border border-primary/20 graph-viewport" data-testid="graph-canvas">
          <SvelteFlow id="yggdrasil" bind:nodes={flowNodes} bind:edges={flowEdges} {nodeTypes} fitView fitViewOptions={{ padding: 0.2 }} minZoom={0.12} maxZoom={2.5} nodeOrigin={[0.5, 0.5]} panOnDrag={[0, 1, 2]} selectionOnDrag={false} nodesDraggable nodesConnectable={false} elementsSelectable onnodeclick={(event) => { if (!metaMode) focus(event.node.id); }}>
            <ViewportPortal target="back">
              <svg class="tree-backdrop" viewBox="-900 -720 1800 1440" aria-hidden="true">
                <defs>
                  <linearGradient id="trunk-gradient" x1="0" x2="0" y1="0" y2="1"><stop offset="0" stop-color="#8b7cff" stop-opacity=".24" /><stop offset=".48" stop-color="#19d3ae" stop-opacity=".45" /><stop offset="1" stop-color="#ffb454" stop-opacity=".22" /></linearGradient>
                  <filter id="tree-glow"><feGaussianBlur stdDeviation="8" result="blur" /><feMerge><feMergeNode in="blur" /><feMergeNode in="SourceGraphic" /></feMerge></filter>
                </defs>
                <g class="realm-hulls">{#each realms as realm (realm.id)}<ellipse cx={realm.x} cy={realm.y} rx={realm.radius} ry={realm.radius * .62} fill={realm.accent} fill-opacity=".055" stroke={realm.accent} stroke-opacity=".16" stroke-dasharray="4 10" /><text x={realm.x} y={realm.y - realm.radius * .58} text-anchor="middle" fill={realm.accent} fill-opacity=".62">{realm.label}</text>{/each}</g>
                <g class="tree-structure" filter="url(#tree-glow)">
                  <path class="tree-trunk" d="M 0 430 C -24 270, 26 120, 0 -40 C -22 -180, 18 -330, 0 -560" />
                  <path class="tree-branch" d="M 0 -220 C -150 -330, -300 -410, -500 -480 M 0 -120 C 140 -260, 300 -350, 520 -430 M 0 20 C -180 -40, -330 -80, -570 -120 M 0 90 C 160 30, 340 10, 570 -40" />
                  <path class="tree-root" d="M 0 390 C -120 470, -300 540, -650 630 M 0 400 C 0 500, 0 590, 0 690 M 0 390 C 120 470, 300 540, 650 630" />
                </g>
                <g class="well-markers"><circle cx="-650" cy="630" r="34" /><circle cx="0" cy="690" r="34" /><circle cx="650" cy="630" r="34" /><text x="-650" y="690" text-anchor="middle">URÐR</text><text x="0" y="750" text-anchor="middle">MÍMIR</text><text x="650" y="690" text-anchor="middle">HVERGELMIR</text></g>
              </svg>
            </ViewportPortal>
            <Background patternColor="rgba(143,151,190,.08)" gap={32} size={1} />
            <Controls position="bottom-left" showFitView showZoom showLock aria-label="Graph viewport controls" />
            <MiniMap position="bottom-right" pannable zoomable nodeColor={(node) => String(node.data.accent ?? '#19d3ae')} nodeStrokeColor="rgba(255,255,255,.25)" maskColor="rgba(7,8,14,.72)" />
          </SvelteFlow>
        </div>
        <div class="flex shrink-0 items-center gap-3 rounded-lg border bg-card/70 px-3 py-2 text-xs">
          <span class="size-2 rounded-full bg-primary shadow-[0_0_10px_var(--accent-glow)]"></span><span class="font-semibold uppercase tracking-wider text-muted-foreground">Selected trunk</span><span class="min-w-0 flex-1 truncate font-mono text-primary" title={selected?.path}>{selected?.path ?? 'Choose a node'}</span>
          {#if selected}<span class="text-muted-foreground">{selected.imports.length} imports · {selected.importers.length} importers · {selected.symbols.length} symbols</span>{#if onopen}<Button variant="ghost" size="sm" onclick={() => onopen?.(selected.path)}>Open in editor</Button>{/if}{/if}
        </div>
      </div>
    {/if}
  </div>
</section>

<style>
  .mapping-state { display: flex; min-height: 180px; align-items: center; justify-content: center; gap: 12px; padding: 24px; text-align: left; }
  .mapping-pulse { width: 12px; height: 12px; border-radius: 999px; background: hsl(var(--primary)); box-shadow: 0 0 0 0 hsl(var(--primary) / .35); animation: mapping-pulse 1.4s ease-out infinite; }
  .guide-card { display: flex; align-items: flex-start; gap: 10px; border: 1px solid hsl(var(--primary) / .2); border-radius: 10px; background: linear-gradient(110deg, hsl(var(--primary) / .08), hsl(var(--card) / .55)); padding: 11px 12px; }
  .guide-icon { display: flex; width: 28px; height: 28px; flex: 0 0 auto; align-items: center; justify-content: center; border-radius: 8px; background: hsl(var(--primary) / .14); color: hsl(var(--primary)); }
  .guide-close { color: hsl(var(--muted-foreground)); font-size: 18px; line-height: 1; }
  .guide-close:hover { color: hsl(var(--foreground)); }
  .guide-reopen { display: inline-flex; align-items: center; gap: 6px; color: hsl(var(--muted-foreground)); font-size: 11px; }
  .guide-reopen:hover { color: hsl(var(--foreground)); }
  .coverage-note { display: flex; align-items: flex-start; gap: 8px; border: 1px solid hsl(38 92% 60% / .28); border-radius: 8px; background: hsl(38 92% 60% / .07); padding: 8px 10px; color: hsl(var(--muted-foreground)); font-size: 11px; }
  .coverage-dot { width: 7px; height: 7px; flex: 0 0 auto; margin-top: 4px; border-radius: 999px; background: #ffb454; box-shadow: 0 0 10px #ffb454; }
  .coverage-details { border: 1px solid hsl(var(--border) / .75); border-radius: 8px; background: hsl(var(--card) / .35); color: hsl(var(--muted-foreground)); font-size: 11px; }
  .coverage-details summary { cursor: pointer; padding: 7px 10px; font-weight: 600; color: hsl(var(--foreground)); }
  .coverage-grid { overflow-x: auto; padding: 0 10px 8px; }
  .coverage-row { display: grid; grid-template-columns: minmax(7rem, 1.4fr) repeat(4, minmax(5rem, .8fr)); gap: 8px; border-top: 1px solid hsl(var(--border) / .45); padding: 6px 0; }
  .coverage-heading { border-top: 0; color: hsl(var(--muted-foreground)); font-size: 9px; text-transform: uppercase; letter-spacing: .12em; }
  @keyframes mapping-pulse { 0% { box-shadow: 0 0 0 0 hsl(var(--primary) / .35); } 70% { box-shadow: 0 0 0 10px hsl(var(--primary) / 0); } 100% { box-shadow: 0 0 0 0 hsl(var(--primary) / 0); } }
  .graph-viewport { background: radial-gradient(circle at 50% 45%, rgba(25,211,174,.09), transparent 44%), #080a10; }
  :global(.tree-backdrop) { position: absolute; left: -900px; top: -720px; width: 1800px; height: 1440px; overflow: visible; pointer-events: none; }
  :global(.tree-backdrop text) { font: 600 11px 'JetBrains Mono', ui-monospace, monospace; letter-spacing: .18em; }
  :global(.tree-trunk), :global(.tree-branch), :global(.tree-root) { fill: none; stroke: url(#trunk-gradient); stroke-linecap: round; }
  :global(.tree-trunk) { stroke-width: 14; opacity: .32; }
  :global(.tree-branch) { stroke-width: 7; opacity: .22; }
  :global(.tree-root) { stroke-width: 9; opacity: .2; }
  :global(.well-markers circle) { fill: rgba(25,211,174,.08); stroke: rgba(25,211,174,.38); stroke-width: 2; filter: url(#tree-glow); }
  :global(.well-markers text) { fill: rgba(25,211,174,.68); font-size: 10px; }
  :global(.graph-viewport .svelte-flow) { --xy-background-color: transparent; --xy-controls-button-background-color: rgba(19,22,32,.92); --xy-controls-button-color: hsl(var(--foreground)); --xy-minimap-background-color: rgba(11,13,21,.9); }
  :global(.graph-viewport .svelte-flow__edge-path) { filter: drop-shadow(0 0 2px rgba(25,211,174,.12)); }
  :global(.graph-viewport .svelte-flow__controls) { border-color: rgba(143,151,190,.22); box-shadow: 0 8px 24px rgba(0,0,0,.24); }
  :global(.graph-viewport .svelte-flow__minimap) { border: 1px solid rgba(143,151,190,.24); border-radius: 10px; overflow: hidden; }
  @media (prefers-reduced-motion: reduce) { .mapping-pulse { animation: none; } }
</style>
