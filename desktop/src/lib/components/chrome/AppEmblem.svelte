<!--
  The brand mark is Mjölnir — the legendary hammer of Thor.
  Designed with Norse beveled geometry, runic channel, and leather-wrapped shaft.
  Crisp down to 16px and scaled with glowing ambient aura.
-->
<script lang="ts">
  let { size = 28 }: { size?: number } = $props();
</script>

<div
  class="relative flex shrink-0 items-center justify-center select-none"
  style={`width:${size}px;height:${size}px;`}
  title="mjolnr"
>
  <div class="emblem-glow"></div>
  <svg width={size} height={size} viewBox="0 0 64 64" fill="none" xmlns="http://www.w3.org/2000/svg" class="relative z-10" aria-hidden="true">
    <!-- Mjölnir Beveled Hammerhead -->
    <path
      class="emblem-head"
      d="M10 13 C10 10.5 12 8.5 14.5 8.5 H49.5 C52 8.5 54 10.5 54 13 V23 C54 25.5 52 27.5 49.5 27.5 H37 V25 H48 V12 H16 V25 H27 V27.5 H14.5 C12 27.5 10 25.5 10 23 Z"
      fill="var(--accent-bright)"
    />
    <path
      class="emblem-face"
      d="M14 12 H50 V24 H14 Z"
      fill="var(--accent-cyan)"
    />
    <!-- Nordic Rune / Power Channel -->
    <path
      class="emblem-rune"
      d="M32 14.5 V21.5 M27 18 H37 M28.5 15.5 L35.5 20.5 M35.5 15.5 L28.5 20.5"
      stroke="var(--bg-void)"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    />
    <!-- Hammer Shaft -->
    <rect
      class="emblem-shaft"
      x="29"
      y="26"
      width="6"
      height="23"
      rx="1.5"
      fill="var(--accent-bright)"
    />
    <!-- Grip Bands -->
    <line x1="29" y1="31" x2="35" y2="31" stroke="var(--bg-void)" stroke-width="1.4" />
    <line x1="29" y1="36" x2="35" y2="36" stroke="var(--bg-void)" stroke-width="1.4" />
    <line x1="29" y1="41" x2="35" y2="41" stroke="var(--bg-void)" stroke-width="1.4" />
    <line x1="29" y1="46" x2="35" y2="46" stroke="var(--bg-void)" stroke-width="1.4" />
    <!-- Pommel & Wrist Loop -->
    <circle class="emblem-pommel" cx="32" cy="50.5" r="3.5" fill="var(--accent-cyan)" />
    <path
      class="emblem-strap"
      d="M32 53.5 C32 57.5 28.5 59.5 26.5 57.5 C24.5 55.5 27.5 52 32 53.5"
      stroke="var(--accent-bright)"
      stroke-width="1.5"
      stroke-linecap="round"
    />
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
  .emblem-head, .emblem-face, .emblem-shaft, .emblem-pommel {
    animation: mjolnir-pulse 4s ease-in-out infinite;
  }
  @keyframes emblem-breathe {
    0%, 100% { opacity: 0.4; transform: scale(1); }
    50% { opacity: 0.75; transform: scale(1.12); }
  }
  @keyframes mjolnir-pulse {
    0%, 100% { opacity: 0.9; }
    50% { opacity: 1; filter: drop-shadow(0 0 4px var(--accent-glow)); }
  }
  @media (prefers-reduced-motion: reduce) {
    .emblem-glow,
    .emblem-head,
    .emblem-face,
    .emblem-shaft,
    .emblem-pommel {
      animation: none;
    }
  }
</style>