<script lang="ts">
  import { Handle, Position, type NodeProps } from '@xyflow/svelte';

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

  let { data, selected }: NodeProps & { data: GraphNodeData } = $props();
</script>

<div
  class="graph-leaf"
  class:graph-leaf-selected={selected}
  class:graph-leaf-cycle={data.inCycle}
  class:graph-leaf-bridge={data.isArticulationPoint}
  style={`--node-accent:${data.accent}`}
  title={data.path}
>
  <Handle type="target" position={Position.Left} class="graph-handle" />
  <span class="graph-leaf-core" aria-hidden="true"></span>
  <span class="graph-leaf-copy">
    <span class="block truncate font-mono text-[10px] font-semibold">{data.label}</span>
    <span class="mt-0.5 block text-[8px] uppercase tracking-[0.16em] text-muted-foreground">{data.language}</span>
  </span>
  <span class="graph-leaf-degree" aria-label={`${data.degree} relationships`}>{data.degree}</span>
  <Handle type="source" position={Position.Right} class="graph-handle" />
</div>

<style>
  .graph-leaf {
    position: relative;
    display: flex;
    width: 154px;
    align-items: center;
    gap: 7px;
    border: 1px solid color-mix(in srgb, var(--node-accent) 48%, transparent);
    border-radius: 70% 28% 70% 28%;
    background: linear-gradient(135deg, color-mix(in srgb, var(--node-accent) 20%, hsl(var(--card))) 0%, hsl(var(--card) / .88) 72%);
    padding: 7px 9px;
    color: hsl(var(--foreground));
    box-shadow: 0 7px 22px color-mix(in srgb, var(--node-accent) 13%, transparent), inset 0 0 18px color-mix(in srgb, var(--node-accent) 7%, transparent);
    transform: rotate(-6deg);
    transition: border-color 180ms ease, box-shadow 180ms ease, transform 180ms ease, opacity 180ms ease;
  }
  .graph-leaf-copy { min-width: 0; flex: 1; transform: rotate(6deg); }
  .graph-leaf-core { width: 8px; height: 8px; flex: 0 0 auto; border-radius: 70% 30% 70% 30%; background: var(--node-accent); box-shadow: 0 0 14px var(--node-accent); transform: rotate(45deg); }
  .graph-leaf-degree { display: flex; height: 17px; min-width: 17px; align-items: center; justify-content: center; border: 1px solid color-mix(in srgb, var(--node-accent) 45%, transparent); border-radius: 999px; color: var(--node-accent); font: 600 8px ui-monospace, monospace; transform: rotate(6deg); }
  .graph-leaf:hover, .graph-leaf-selected { border-color: var(--node-accent); box-shadow: 0 0 0 1px color-mix(in srgb, var(--node-accent) 48%, transparent), 0 0 28px color-mix(in srgb, var(--node-accent) 25%, transparent); transform: rotate(0deg) scale(1.06); z-index: 2; }
  .graph-leaf-selected .graph-leaf-copy, .graph-leaf-selected .graph-leaf-degree { transform: rotate(0deg); }
  .graph-leaf-cycle { border-style: dashed; }
  .graph-leaf-bridge { box-shadow: 0 0 0 2px color-mix(in srgb, #fbbf24 50%, transparent), 0 0 25px color-mix(in srgb, #fbbf24 22%, transparent); }
  :global(.graph-handle) { width: 1px; height: 1px; border: 0; opacity: 0; pointer-events: none; }
  @media (prefers-reduced-motion: reduce) { .graph-leaf { transition: none; } }
</style>
