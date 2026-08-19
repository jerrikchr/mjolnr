<!--
  The small breathing status dot used across the header (connection) and
  sidebar (accounts/worktrees/fleet, §E2.5 Stage 3) — one definition so every
  surface agrees on what "active"/"verified"/"idle"/"attention" look like.
-->
<script lang="ts">
  let {
    state,
    size = 7
  }: { state: 'active' | 'verified' | 'idle' | 'attention'; size?: number } = $props();
</script>

<div class={`status-orb status-orb-${state}`} style={`width:${size}px;height:${size}px;`}></div>

<style>
  .status-orb {
    border-radius: 50%;
    flex-shrink: 0;
  }
  .status-orb-active {
    background: var(--accent-cyan);
    box-shadow: 0 0 6px var(--accent-glow);
    animation: orb-breathe 2s ease-in-out infinite;
  }
  .status-orb-verified {
    background: var(--gov-verified);
    box-shadow: 0 0 4px var(--gov-verified-border);
  }
  .status-orb-idle {
    background: var(--text-tertiary);
  }
  .status-orb-attention {
    background: var(--gov-approval);
    box-shadow: 0 0 6px var(--gov-approval-glow);
    animation: orb-breathe 1.8s ease-in-out infinite;
  }
  @keyframes orb-breathe {
    0%, 100% { opacity: 0.6; transform: scale(1); }
    50% { opacity: 1; transform: scale(1.15); }
  }
  @media (prefers-reduced-motion: reduce) {
    .status-orb-active,
    .status-orb-attention {
      animation: none;
      opacity: 1;
    }
  }
</style>
