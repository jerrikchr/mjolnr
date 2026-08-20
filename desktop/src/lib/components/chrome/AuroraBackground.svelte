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

  <!-- Soft vignette fade towards bottom -->
  <div class="absolute inset-0 bg-gradient-to-b from-transparent via-transparent to-background/80 pointer-events-none"></div>
</div>

<style>
  .aurora-container {
    filter: blur(65px);
    opacity: 0.55;
    transition: opacity 0.5s ease;
    animation: aurora-global-breathe 14s ease-in-out infinite alternate;
  }

  :global(.dark) .aurora-container {
    opacity: 0.60;
  }

  :global(:not(.dark)) .aurora-container {
    opacity: 0.20;
    filter: blur(80px);
  }

  .aurora-orb {
    position: absolute;
    border-radius: 9999px;
    will-change: transform, opacity;
    transform: translate3d(0, 0, 0);
  }

  /* Emerald Lightning Wave */
  .aurora-orb-1 {
    top: -8%;
    left: 15%;
    width: 60vw;
    height: 55vh;
    background: radial-gradient(ellipse at center, rgba(16, 185, 129, 0.7) 0%, rgba(5, 150, 105, 0.35) 50%, transparent 75%);
    animation: aurora-drift-1 11s ease-in-out infinite alternate;
  }

  /* Celestial Cyan Wave */
  .aurora-orb-2 {
    top: -2%;
    right: 10%;
    width: 55vw;
    height: 50vh;
    background: radial-gradient(ellipse at center, rgba(6, 182, 212, 0.65) 0%, rgba(8, 145, 178, 0.3) 50%, transparent 75%);
    animation: aurora-drift-2 13s ease-in-out infinite alternate;
    animation-delay: -3s;
  }

  /* Indigo/Violet Cosmic Flow */
  .aurora-orb-3 {
    top: 5%;
    left: 25%;
    width: 50vw;
    height: 45vh;
    background: radial-gradient(ellipse at center, rgba(99, 102, 241, 0.55) 0%, rgba(139, 92, 246, 0.25) 50%, transparent 75%);
    animation: aurora-drift-3 15s ease-in-out infinite alternate;
    animation-delay: -6s;
  }

  /* Boreal Mint Sparkle Wave */
  .aurora-orb-4 {
    top: 0%;
    left: 5%;
    width: 45vw;
    height: 40vh;
    background: radial-gradient(ellipse at center, rgba(52, 211, 153, 0.6) 0%, rgba(16, 185, 129, 0.25) 50%, transparent 75%);
    animation: aurora-drift-4 9s ease-in-out infinite alternate;
    animation-delay: -2s;
  }

  @keyframes aurora-global-breathe {
    0% {
      transform: scale(1);
      filter: blur(65px) hue-rotate(0deg);
    }
    50% {
      transform: scale(1.05);
      filter: blur(75px) hue-rotate(10deg);
    }
    100% {
      transform: scale(0.98);
      filter: blur(60px) hue-rotate(-8deg);
    }
  }

  @keyframes aurora-drift-1 {
    0% {
      transform: translate3d(0, 0, 0) scale(1) rotate(0deg);
      opacity: 0.9;
    }
    50% {
      transform: translate3d(10vw, 8vh, 0) scale(1.25) rotate(16deg);
      opacity: 1;
    }
    100% {
      transform: translate3d(-8vw, 14vh, 0) scale(0.9) rotate(-12deg);
      opacity: 0.8;
    }
  }

  @keyframes aurora-drift-2 {
    0% {
      transform: translate3d(0, 0, 0) scale(1) rotate(0deg);
      opacity: 0.85;
    }
    50% {
      transform: translate3d(-12vw, 10vh, 0) scale(1.2) rotate(-18deg);
      opacity: 1;
    }
    100% {
      transform: translate3d(7vw, 5vh, 0) scale(1.3) rotate(14deg);
      opacity: 0.75;
    }
  }

  @keyframes aurora-drift-3 {
    0% {
      transform: translate3d(0, 0, 0) scale(1) rotate(0deg);
      opacity: 0.75;
    }
    50% {
      transform: translate3d(-7vw, -6vh, 0) scale(1.3) rotate(12deg);
      opacity: 1;
    }
    100% {
      transform: translate3d(9vw, 8vh, 0) scale(0.85) rotate(-14deg);
      opacity: 0.8;
    }
  }

  @keyframes aurora-drift-4 {
    0% {
      transform: translate3d(0, 0, 0) scale(1) rotate(0deg);
      opacity: 0.9;
    }
    50% {
      transform: translate3d(8vw, 6vh, 0) scale(1.15) rotate(-10deg);
      opacity: 1;
    }
    100% {
      transform: translate3d(-6vw, 10vh, 0) scale(1.28) rotate(18deg);
      opacity: 0.85;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .aurora-container,
    .aurora-orb {
      animation: none;
    }
  }
</style>
