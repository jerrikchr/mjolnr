<!--
  The brand mark is Mjölnir — the legendary Norse hammer forged by Sindri and Brokkr.
  Authentic Norse flared hammerhead, carved rune channel, brass collar, cross-wrapped grip,
  and braided pommel loop with dynamic celestial aurora glow.
-->
<script lang="ts">
  let { size = 28 }: { size?: number } = $props();
</script>

<div
  class="relative flex shrink-0 items-center justify-center select-none emblem-wrap"
  style={`width:${size}px;height:${size}px;`}
  title="mjolnr — The Hammer of Code"
>
  <div class="emblem-glow"></div>
  <svg
    width={size}
    height={size}
    viewBox="0 0 64 64"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    class="relative z-10 filter drop-shadow-md"
    aria-hidden="true"
  >
    <defs>
      <!-- Hammerhead Outer Bevel Gradient -->
      <linearGradient id="mjolnir-head-grad" x1="8" y1="8" x2="56" y2="30" gradientUnits="userSpaceOnUse">
        <stop offset="0%" stop-color="#10b981" />
        <stop offset="50%" stop-color="#059669" />
        <stop offset="100%" stop-color="#047857" />
      </linearGradient>

      <!-- Hammer Face Center Plate Gradient -->
      <linearGradient id="mjolnir-face-grad" x1="16" y1="12" x2="48" y2="28" gradientUnits="userSpaceOnUse">
        <stop offset="0%" stop-color="#34d399" />
        <stop offset="100%" stop-color="#059669" />
      </linearGradient>

      <!-- Rune / Lightning Glow Gradient -->
      <linearGradient id="mjolnir-rune-grad" x1="24" y1="14" x2="40" y2="26" gradientUnits="userSpaceOnUse">
        <stop offset="0%" stop-color="#ffffff" />
        <stop offset="100%" stop-color="#a7f3d0" />
      </linearGradient>

      <!-- Shaft Core Gradient -->
      <linearGradient id="mjolnir-shaft-grad" x1="28" y1="30" x2="36" y2="52" gradientUnits="userSpaceOnUse">
        <stop offset="0%" stop-color="#064e3b" />
        <stop offset="50%" stop-color="#022c22" />
        <stop offset="100%" stop-color="#064e3b" />
      </linearGradient>

      <!-- Collar & Pommel Brass/Bronze Accent -->
      <linearGradient id="mjolnir-brass-grad" x1="26" y1="26" x2="38" y2="56" gradientUnits="userSpaceOnUse">
        <stop offset="0%" stop-color="#6ee7b7" />
        <stop offset="100%" stop-color="#059669" />
      </linearGradient>
    </defs>

    <!-- HAMMERHEAD (Authentic Norse T-Shape with Flared Cheeks and Top Crown) -->
    <!-- Top Crown Peak -->
    <path
      d="M26 8.5 C29 6.5 35 6.5 38 8.5 L40 10.5 H24 L26 8.5 Z"
      fill="url(#mjolnir-brass-grad)"
    />

    <!-- Main Solid Hammerhead Geometry with Beveled Lower Flanges -->
    <path
      d="M10 11 C10 9.5 11.5 8.5 13 8.5 H51 C52.5 8.5 54 9.5 54 11 L56 22 C56 25 53 27 50 26.5 L40 25 L37 27 H27 L24 25 L14 26.5 C11 27 8 25 8 22 L10 11 Z"
      fill="url(#mjolnir-head-grad)"
      stroke="#047857"
      stroke-width="1.2"
      stroke-linejoin="round"
    />

    <!-- Inner Recessed Relief Plate -->
    <path
      d="M14 12.5 H50 L52 21 C52 22.5 50.5 23.5 48.5 23 L38 22 L36 24 H28 L26 22 L15.5 23 C13.5 23.5 12 22.5 12 21 L14 12.5 Z"
      fill="url(#mjolnir-face-grad)"
      opacity="0.9"
    />

    <!-- Norse Tiwaz/Lightning Runic Inscription (Thor's Divine Power) -->
    <path
      d="M32 12 V22.5 M26 15 L32 18.5 L38 15 M28 20.5 L32 18 L36 20.5"
      stroke="url(#mjolnir-rune-grad)"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    />

    <!-- Neck Collar (Forged Iron Ring between Head and Haft) -->
    <rect
      x="27"
      y="26"
      width="10"
      height="3.5"
      rx="1"
      fill="url(#mjolnir-brass-grad)"
      stroke="#065f46"
      stroke-width="0.8"
    />

    <!-- Hammer Haft / Shaft -->
    <rect
      x="28.5"
      y="29.5"
      width="7"
      height="22"
      rx="1.2"
      fill="url(#mjolnir-shaft-grad)"
      stroke="#047857"
      stroke-width="0.8"
    />

    <!-- Leather Diamond Cross-Hatch Wraps on Grip -->
    <path
      d="M28.5 33 L35.5 36 M28.5 39 L35.5 42 M28.5 45 L35.5 48 M35.5 33 L28.5 36 M35.5 39 L28.5 42 M35.5 45 L28.5 48"
      stroke="#10b981"
      stroke-width="1.1"
      stroke-linecap="round"
      opacity="0.75"
    />

    <!-- Pommel Base Ring -->
    <rect
      x="27"
      y="51.5"
      width="10"
      height="3.5"
      rx="1.2"
      fill="url(#mjolnir-brass-grad)"
      stroke="#065f46"
      stroke-width="0.8"
    />

    <!-- Pommel Ring and Hanging Wrist Lanyard Loop -->
    <circle
      cx="32"
      cy="56"
      r="2.8"
      stroke="url(#mjolnir-brass-grad)"
      stroke-width="1.4"
      fill="none"
    />
    <path
      d="M32 58.5 C32 62.5 27 63 26 60 C25 57 29.5 56.5 32 58"
      stroke="#34d399"
      stroke-width="1.3"
      stroke-linecap="round"
      fill="none"
      opacity="0.85"
    />
  </svg>
</div>

<style>
  .emblem-wrap {
    display: inline-flex;
    transition: transform 0.2s cubic-bezier(0.34, 1.56, 0.64, 1);
  }

  .emblem-wrap:hover {
    transform: scale(1.08) rotate(-2deg);
  }

  .emblem-glow {
    position: absolute;
    inset: -20%;
    border-radius: 9999px;
    background: radial-gradient(circle at center, rgba(16, 185, 129, 0.35) 0%, rgba(52, 211, 153, 0.15) 50%, transparent 75%);
    filter: blur(8px);
    pointer-events: none;
    animation: mjolnir-aura 4s ease-in-out infinite alternate;
  }

  @keyframes mjolnir-aura {
    0% {
      opacity: 0.45;
      transform: scale(0.92);
    }
    100% {
      opacity: 0.85;
      transform: scale(1.1);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .emblem-glow {
      animation: none;
    }
  }
</style>