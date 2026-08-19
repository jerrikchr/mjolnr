# smed TUI visual & design system contract

smed is a **governed agent workspace**. Ratatui is its compact terminal client,
not a desktop-interface imitation. Its visual identity balances mission-control
telemetry with a soft, modern, accessible design system while keeping the
governed loop fast and legible.

The Tauri client selected in
[`ADR 0003`](./adr/0003-shared-rust-core-tui-tauri-clients.md) will have its own
visual contract. Shared colours or vocabulary do not make the two clients share
layout rules.

---

## 1. Design System Philosophy

Traditional terminal applications often suffer from high visual noise, rigid ASCII boxes, harsh matrix greens, and dense, unpadded layouts that intimidate non-terminal users. smed's Soft TUI Design System solves this with five core principles:

1. **Soft Surfaces & Rounded Geometry:** Uses rounded card borders (`BorderType::Rounded`), subtle background elevation tiers (`Void`, `SurfaceCanvas`, `SurfaceElevated`), and generous padding (`padding(1)`) to create visual breathing room similar to modern web/desktop tools (Linear, Raycast, Notion).
2. **Harmonious Palette & Soft Accents:** Muted slate/zinc canvases paired with soft pastel semantic accents (Mint for `Verified`, Amber/Honey for `Approval`, Coral/Rose for `Refusal`, Sky Blue/Iris for `Proposals`). Harsh, pure `#FFFFFF` white or bright neon `#00FF00` green text is avoided in favor of warm, readable tones (`#CFE0DE` Frost / `#E2E8F0` Slate).
3. **Typographic Hierarchy & Friendly Badges:** Clear visual distinction between titles, body text, metadata, and controls using bold weights, dimmed secondary text, and rounded pill badges (`[ 🟢 Active ]`, `[ main ]`, `[ ✦ Proposed ]`).
4. **Intuitive Discovery over Memorization:** Every surface exposes clear visual controls, mouse support (clickable tabs, scroll wheel, focus selection), and a universal `Ctrl+P` jump palette so non-terminal users never feel lost or forced to memorize obscure slash commands.
5. **Fail-Closed Governance Integrity:** Visual softness never weakens governance. Policy gates, approval requests, and recovery decisions remain unmistakable, high-contrast, blocking interactions with explicit diff/effect previews.

### Restraint rule

A capability does not earn permanent screen space merely because it exists.
Rails, panels, and inspectors are contextual: show them when the current action
or decision needs them, then return the space to the primary task. The TUI must
not delay Tauri by attempting to implement every rich-workspace surface first.

---

## 2. Elevation & Surface Tiers

smed uses three background elevation tiers to structure visual hierarchy without cluttering the screen with nested boxes:

| Tier | Dark Role (Noir / Soft Dark) | Light Role (Zeppi Light) | Meaning & Usage |
|---|---|---|---|
| **Tier 0: Canvas** | `#05090F` (Void) | `#F8FAFC` (Paper) | Primary background canvas for the entire terminal frame. |
| **Tier 1: Card / Panel** | `#0A121D` (Surface) | `#FFFFFF` (Surface Light) | Raised cards, work rail items, primary surface views, tool execution blocks. |
| **Tier 2: Elevated Modal** | `#131F30` (Elevated) | `#F1F5F9` (Elevated Light) | High-priority modals (Jump Palette `Ctrl+P`, Approval Gates, Recovery Overlays). |

---

## 3. Semantic Color Palettes

Theme rendering code queries semantic color roles rather than hardcoding RGB literals (`AGENTS.md` §10). Shipped themes include `noir-soft` (default), `zeppi`, `zeppi-light`, `slate`, and `mono`.

### 3.1 Default Theme: `noir-soft`

| Role | RGB | Hex | Brand Name | Purpose / Meaning |
|---|---:|---|---|---|
| **Canvas** | `5, 9, 15` | `#05090F` | Void | Terminal canvas |
| **Surface** | `10, 18, 29` | `#0A121D` | Surface Slate | Card backgrounds & primary panels |
| **Elevated** | `19, 31, 48` | `#131F30` | Elevated Navy | Modals, jump palette, active focus card |
| **Text** | `207, 224, 222` | `#CFE0DE` | Frost | Primary readable text |
| **Muted** | `100, 116, 139` | `#64748B` | Muted Slate | Secondary labels, hints, metadata |
| **Subtle Border**| `30, 41, 59` | `#1E293B` | Border Dark | Hairline rounded card borders |
| **Proposal** | `56, 189, 248` | `#38BDF8` | Soft Sky | Model proposals, streaming deltas |
| **Approval** | `251, 191, 36` | `#FBBF24` | Soft Amber | Policy gates, pending decisions |
| **Verified** | `52, 211, 153` | `#34D399` | Soft Mint | Verified outcomes, passed tests |
| **Refusal** | `248, 113, 113` | `#F87171` | Soft Coral | Refusals, errors, failed checks |
| **Focus Ring** | `129, 140, 248` | `#818CF8` | Soft Iris | Active card or focused input highlight |

### 3.2 Light Theme: `zeppi-light`

| Role | RGB | Hex | Purpose / Meaning |
|---|---:|---|---|
| **Canvas** | `248, 250, 252` | `#F8FAFC` | Clean light background |
| **Surface** | `255, 255, 255` | `#FFFFFF` | Crisp white card surface |
| **Elevated** | `241, 245, 249` | `#F1F5F9` | Raised popups & active tabs |
| **Text** | `15, 23, 42` | `#0F172A` | Deep charcoal primary text |
| **Muted** | `100, 116, 139` | `#64748B` | Readable secondary text |
| **Subtle Border**| `226, 232, 240` | `#E2E8F0` | Soft gray card outlines |
| **Focus Ring** | `99, 102, 241` | `#6366F1` | Soft indigo focus ring |

---

## 4. Typography, Badges & Iconography

### 4.1 Rounded Badges & Pills
- **Work Status Pills:** `[ 🟢 Active ]`, `[ 🟡 Needs Decision ]`, `[ 🔵 Reviewing ]`, `[ 🔴 Failed ]`, `[ ⚪ Draft ]`.
- **Branch / Worktree Badges:** `[  main: feature-auth ]`, `[ 🌲 worktree: /tmp/smed-12 ]`.
- **Policy Tier Pills:** `[ 🛡️ Ask ]`, `[ ⚡ Full-Auto ]`, `[ 🔒 Strict ]`.

### 4.2 Unicode Icon Standard
smed uses clean, widely supported Unicode symbols with fallbacks for basic ASCII terminals:

| Feature | Primary Unicode Symbol | ASCII Fallback |
|---|:---:|:---:|
| Active Agent | `✦` | `*` |
| Verified Check | `✓` | `+` |
| Refusal / Error | `✕` | `x` |
| Pending Decision | `⚡` | `!` |
| Folder Tree | `📁` / `📄` | `[D]` / `[F]` |
| Branch | `` or `⎇` | `b:` |
| Upward Rollup | `↳` | `->` |

---

## 5. Layout Restraint & Chrome Contract

1. **Terminal Edge is the Boundary:** No outer full-screen box enclosing the terminal frame. The terminal boundary is the natural edge.
2. **Space & Elevation before Rules:** Blank lines, surface background colors, and padding group content before drawing lines.
3. **Rounded Hairline Borders:** Card containers use `BorderType::Rounded` with `Subtle Border` styling (`#1E293B`). Borders are never brightly colored except when indicating active keyboard focus (`Focus Ring` `#818CF8`).
4. **Single Gate Accent:** Accent colors are reserved for actionable states. When an approval gate is open, it dims the background canvas to draw complete focus to the decision.
5. **Responsive Surface Switcher:** Wide screens (≥120 cols) display the Work Rail, Primary Surface, and Attention Rail concurrently. Medium and Narrow screens (<120 cols) provide a soft top tab switcher with mouse-clickable tab targets.

---

## 6. Non-Terminal-Native Comfort & Accessibility

To make smed feel friendly to non-CLI users:

1. **Mouse & Scroll Wheel Support:**
   - Tabs, list items, jump palette items, and buttons are fully clickable via terminal mouse events.
   - Mouse wheel scrolling operates seamlessly over signal logs, diffs, and work lists.
2. **Universal Jump Surface (`Ctrl+P`):**
   - Non-CLI users do not need to memorize slash commands. Pressing `Ctrl+P` opens a visual command & navigation palette with fuzzy search.
   - `Ctrl+J` was the original specification and is **not** available: it is the composer's newline on terminals that cannot report `Shift+Enter`, and editing keys outrank navigation keys.
3. **Visible Key Hints & Action Footers:**
   - Soft footer line displays context-sensitive keyboard hints: `[Ctrl+P] Jump  [Ctrl+A] Attention  [Esc] Back  [F1] Help`.
4. **Color-Blind & High-Contrast Guarantees:**
   - Color is never used alone to encode state (`AGENTS.md` §11 law 9). Every status carries an explicit text label and icon badge. The `mono` theme validates this contract mechanically.

---

## 7. Key-Action Contract

Physical key events resolve to semantic actions in a context-aware keymap (`src/tui/keymap.rs`):

| Key | Idle / Work Surface | Active Run | Approval Gate | Jump Palette (`Ctrl+P`) |
|---|---|---|---|---|
 | `Ctrl+P` | Open Jump Palette | Open Jump Palette | No action (Gate focused) | Close Jump Palette |
 | (`/plugins`) | Inspect plugins — `[THIRDPARTY · EXECUTE]` badge (`src/tui/plugins.rs:26-38`, hint `src/main.rs:563`) | — | — | — |
| `Ctrl+PgUp` / `Ctrl+PgDn` | Previous / next primary surface | Previous / next primary surface | No action | No action |
| `Ctrl+A` (empty directive) | Go to Attention queue | Go to Attention queue | No action | Insert text into query |
| `Ctrl+J` | Insert newline | Insert newline | No action | Close Jump Palette |
| `Ctrl-C` (1st press) | Clear input / Arm exit | Request interrupt | Request interrupt | Close palette |
| `Ctrl-C` (2nd within 750ms) | Quit | Quit | Quit | Quit |
| `Esc` | Return focus / Close overlay | Request interrupt | Request interrupt | Close palette |
| `Enter` | Submit directive / Activate item | Lock (Queue directive) | No action | Execute selection |
| `y` / `n` / `a` | Insert text | Insert text | Approve / Deny / Approve Session | Insert text into query |

---

## 8. Prohibitions

1. **No Cold Matrix Noise:** Avoid pure neon green text on harsh black backgrounds unless explicitly requested via `/theme matrix`.
2. **No Unpadded Dense Text:** Paragraphs and cards must include padding to avoid text touching borders.
3. **No Unlabelled Color States:** Color reinforces state, but a text label and glyph must always accompany it.
4. **No Decorative Animation:** Spinners and carets animate only while work is active. No background eye-candy motion.
5. **No Hidden State Modes:** Input area displays exact mode (`Directive Input`, `Jump Query`, `Diff Filter`).
6. **No Panel Accretion:** Do not add an always-visible panel without proving
   that it is useful across the majority of frames at that width.
7. **No Desktop Mimicry:** Canvas, rich onboarding, and persistent multi-object
   workspace composition belong primarily in Tauri. The TUI exposes their
   runtime state through focused views when useful.
