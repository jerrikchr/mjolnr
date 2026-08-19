<!--
  The "is it working" equaliser beside the loop-state indicator. `active`
  should reflect real streaming/run activity (`snap.runActive`) — never
  animate it to imply work that is not actually happening.
-->
<script lang="ts">
  let { active = false }: { active?: boolean } = $props();
</script>

<div class="flex h-3 items-center gap-0.5 text-current">
  {#each [0, 1, 2, 3] as index (index)}
    <div
      class="activity-bar"
      class:activity-bar-active={active}
      style={`animation-delay:${index * 0.12}s`}
    ></div>
  {/each}
</div>

<style>
  .activity-bar {
    width: 2px;
    background: currentColor;
    border-radius: 1px;
    height: 3px;
    transition: height var(--t-normal) var(--ease);
  }
  .activity-bar-active {
    animation: bar-wave 0.9s ease-in-out infinite;
  }
  @keyframes bar-wave {
    0%, 100% { height: 3px; }
    50% { height: 11px; }
  }
  @media (prefers-reduced-motion: reduce) {
    .activity-bar-active {
      animation: none;
      height: 8px;
    }
  }
</style>
