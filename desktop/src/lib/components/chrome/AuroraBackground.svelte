<!--
  AuroraBackground: A subtle, ethereal Norse Northern Lights backdrop
  using zero-dependency GPU-accelerated CSS mesh ribbons and Gaussian blur.
  Zero CPU impact, fully theme-adaptive, and respects reduced motion.
-->
<script lang="ts">
  import { cn } from '$lib/utils';

  let {
    class: className = ''
  }: {
    class?: string;
  } = $props();
</script>

<div
  class={cn(
    'absolute inset-0 pointer-events-none overflow-hidden select-none z-0',
    className
  )}
  aria-hidden="true"
>
  <!-- Background Base Veil -->
  <div class="absolute inset-0 bg-radial-[ellipse_at_top_center] from-transparent via-background/40 to-background z-10"></div>

  <!-- Animated Aurora Mesh Ribbons Container -->
  <div class="aurora-container absolute inset-0">
    <!-- Aurora Ribbon 1: Emerald Lightning Wave -->
    <div class="aurora-orb aurora-orb-1"></div>

    <!-- Aurora Ribbon 2: Celestial Arctic Cyan Ribbon -->
    <div class="aurora-orb aurora-orb-2"></div>

    <!-- Aurora Ribbon 3: Night Sky Indigo/Violet Undertone -->
    <div class="aurora-orb aurora-orb-3"></div>

    <!-- Aurora Ribbon 4: Boreal Mint Spark -->
    <div class="aurora-orb aurora-orb-4"></div>
  </div>
</div>

<style>
  .aurora-container {
    filter: blur(80px);
    opacity: 0.15;
    transition: opacity 0.5s ease;
  }

  :global(.dark) .aurora-container {
    opacity: 0.18;
  }

  :global(:not(.dark)) .aurora-container {
    opacity: 0.06;
    filter: blur(100px);
  }

  .aurora-orb {
    position: absolute;
    border-radius: 9999px;
    will-change: transform;
    transform: translate3d(0, 0, 0);
  }

  /* Emerald Wave */
  .aurora-orb-1 {
    top: -15%;
    left: 20%;
    width: 65vw;
    height: 45vh;
    background: radial-gradient(circle, rgba(16, 185, 129, 0.8) 0%, rgba(5, 150, 105, 0.4) 60%, transparent 80%);
    animation: aurora-drift-1 28s cubic-bezier(0.4, 0, 0.2, 1) infinite alternate;
  }

  /* Celestial Cyan Wave */
  .aurora-orb-2 {
    top: -10%;
    right: 15%;
    width: 55vw;
    height: 40vh;
    background: radial-gradient(circle, rgba(6, 182, 212, 0.75) 0%, rgba(8, 145, 178, 0.35) 60%, transparent 80%);
    animation: aurora-drift-2 34s cubic-bezier(0.4, 0, 0.2, 1) infinite alternate;
  }

  /* Indigo/Violet Depth */
  .aurora-orb-3 {
    top: 5%;
    left: 35%;
    width: 50vw;
    height: 35vh;
    background: radial-gradient(circle, rgba(99, 102, 241, 0.6) 0%, rgba(139, 92, 246, 0.25) 60%, transparent 80%);
    animation: aurora-drift-3 40s cubic-bezier(0.4, 0, 0.2, 1) infinite alternate;
  }

  /* Boreal Mint Spark */
  .aurora-orb-4 {
    top: -5%;
    left: 5%;
    width: 45vw;
    height: 30vh;
    background: radial-gradient(circle, rgba(52, 211, 153, 0.65) 0%, rgba(16, 185, 129, 0.2) 60%, transparent 80%);
    animation: aurora-drift-4 24s cubic-bezier(0.4, 0, 0.2, 1) infinite alternate;
  }

  @keyframes aurora-drift-1 {
    0% {
      transform: translate3d(0, 0, 0) scale(1) rotate(0deg);
    }
    50% {
      transform: translate3d(8vw, 6vh, 0) scale(1.15) rotate(12deg);
    }
    100% {
      transform: translate3d(-6vw, 10vh, 0) scale(0.95) rotate(-8deg);
    }
  }

  @keyframes aurora-drift-2 {
    0% {
      transform: translate3d(0, 0, 0) scale(1) rotate(0deg);
    }
    50% {
      transform: translate3d(-10vw, 8vh, 0) scale(1.1) rotate(-14deg);
    }
    100% {
      transform: translate3d(5vw, 4vh, 0) scale(1.2) rotate(10deg);
    }
  }

  @keyframes aurora-drift-3 {
    0% {
      transform: translate3d(0, 0, 0) scale(1) rotate(0deg);
    }
    50% {
      transform: translate3d(-5vw, -4vh, 0) scale(1.2) rotate(8deg);
    }
    100% {
      transform: translate3d(7vw, 6vh, 0) scale(0.9) rotate(-10deg);
    }
  }

  @keyframes aurora-drift-4 {
    0% {
      transform: translate3d(0, 0, 0) scale(1) rotate(0deg);
    }
    50% {
      transform: translate3d(6vw, 5vh, 0) scale(1.05) rotate(-6deg);
    }
    100% {
      transform: translate3d(-4vw, 8vh, 0) scale(1.18) rotate(14deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .aurora-orb {
      animation: none;
    }
  }
</style>
