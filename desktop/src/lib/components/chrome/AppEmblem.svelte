<!--
  The brand mark is an anvil — smed is Danish for "smith". It is deliberately
  legible at 16px: the block + horn silhouette stays crisp down to icon size.

  Bespoke chrome with no shadcn equivalent, so it reads the raw Obsidian
  Cyan/Paper Cyan tokens directly (see app.css) rather than routing through a
  Tailwind utility invented for this one component.
-->
<script lang="ts">
  let { size = 28 }: { size?: number } = $props();
</script>

<div
  class="relative flex shrink-0 items-center justify-center"
  style={`width:${size}px;height:${size}px;`}
  title="smed — the smith"
>
  <div class="emblem-glow"></div>
  <svg width={size} height={size} viewBox="0 0 64 64" class="relative z-10" aria-hidden="true">
    <path class="emblem-anvil" d="M15 12 H49 V24 H40 V40 H54 V52 H10 V40 H24 V24 H15 Z" />
    <path class="emblem-anvil" d="M15 16 L6 28 H15 Z" />
  </svg>
</div>

<style>
  .emblem-glow {
    position: absolute;
    inset: -6px;
    border-radius: 50%;
    background: radial-gradient(circle, var(--accent-glow) 0%, transparent 65%);
    animation: emblem-breathe 4s ease-in-out infinite;
    pointer-events: none;
  }
  .emblem-anvil {
    fill: var(--accent-bright);
    animation: anvil-pulse 4s ease-in-out infinite;
  }
  @keyframes emblem-breathe {
    0%, 100% { opacity: 0.35; transform: scale(1); }
    50% { opacity: 0.65; transform: scale(1.1); }
  }
  @keyframes anvil-pulse {
    0%, 100% { opacity: 0.85; }
    50% { opacity: 1; filter: drop-shadow(0 0 3px var(--accent-glow)); }
  }
  @media (prefers-reduced-motion: reduce) {
    .emblem-glow,
    .emblem-anvil {
      animation: none;
    }
  }
</style>