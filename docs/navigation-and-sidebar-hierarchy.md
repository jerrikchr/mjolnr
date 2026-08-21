# Desktop Navigation & Sidebar Information Architecture

This document defines the authoritative information architecture and sidebar hierarchy for the **mjolnr Desktop Client** (Tauri + SvelteKit), synthesizing proven patterns from Codex and Orca with mjolnr's deterministic governance.

---

## 1. The Core Hierarchy: Workspace → Project → Sessions → Worktrees & Fleet

Developers organize mental context around projects and repositories. Flattening all conversations into a single global list creates visual chaos and loses context. 

mjolnr establishes a strict 4-level structural hierarchy:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. Workspace / Organization (Global context, provider accounts, settings)   │
│   └── 2. Project (Repository / Root Paths / Multi-folder context)           │
│         └── 3. Sessions (Conversations, Tasks, Durable Event Lineages)       │
│               └── 4. Worktrees & Fleet (Local subagents & Cloud Jules runs) │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Complete Sidebar Layout & Component Breakdown

```
┌──────────────────────────────────────────────────────────────────┐
│ [Emblem] Workspace / Profile Name  [🔍 ⌘K] [🔔 Notifications]   │
├──────────────────────────────────────────────────────────────────┤
│ ➕ New Chat (⌘N)                                                 │
│ 🔀 Pull Requests                                     (2 pending) │
│ 🌐 Sites & Studio Previews                           (1 active)  │
│ ⏰ Scheduled Cloud Tasks                             (3 daily)   │
│ 🧩 Plugins & MCP Connections                         (8 enabled) │
├──────────────────────────────────────────────────────────────────┤
│ PROJECTS                                                         │
│                                                                  │
│ 📁 ibf-saigon                                                    │
│ 📁 buddha-bar (active)                                           │
│ 📁 simon-says                                          [...] [➕]│
│   ├─ 💬 Implement auth token refresh                             │
│   ├─ 💬 Fix CI build failure in vitest                           │
│   ├─ 💬 Performance benchmark run                                │
│   │    └─ 🌿 mjolnr/sub-cache-bench (Running)                    │
│   │    └─ ☁️ Google Jules: Sentinel Scan (In Progress)           │
│   └─ Show more (15 older tasks)...                               │
│ 📁 zWap                                                          │
│ 📁 Archi                                                         │
├──────────────────────────────────────────────────────────────────┤
│ ▶ RECENTS (Archived / Inactive Projects)                         │
├──────────────────────────────────────────────────────────────────┤
│ 👤 User Profile (Jerrik C.)      🎙️ Voice / Mic      ⚙️ Soul/Gov │
└──────────────────────────────────────────────────────────────────┘
```

---

## 3. Detailed Section Specifications

### 3.1 Header & Global Action Rails
* **Workspace Selector**: Dropdown showing active organization/workspace, with global search trigger (`⌘K` Command Palette) and notification badge.
* **`New Chat` (`⌘N`)**: Global button to instantly spin up a new session in the currently selected project or open a blank scratchpad.
* **Cross-Project Views (Global Items)**:
  * **Pull Requests (`🔀`)**: Aggregated view of open PRs across all workspace projects (GitHub/Linear task integrations).
  * **Sites & Previews (`🌐`)**: Active local preview servers, responsive iframes, and deployment previews (Vercel).
  * **Scheduled Cloud Tasks (`⏰`)**: Recurring cron jobs and cloud maintenance agents (Google Jules daily security scans, weekly optimizers).
  * **Plugins & MCP (`🧩`)**: Browseable connections and marketplace manager for MCP servers (Neon, Supabase, Tinybird, v0) and ADR-0016 JSON-RPC plugins.

---

### 3.2 Projects Section (Primary Operational Group)
Each project corresponds to a primary git repository or workspace folder:

* **Project Item State**:
  * Displays repository/folder icon (`📁`), project name, and active status orb.
  * Hover reveals quick actions:
    * `+`: Launch a new session directly bound to this project.
    * `...`: Project options popover.
* **Project Context Card (Hover / Popover)**:
  * **Multi-Root Binding**: Shows associated paths (e.g. `~/Code/simon-says` and `~/Code/simon-says-research`).
  * **Task Count**: Summary badge (e.g. `18 tasks`).
  * **Edit Project**: Configures repo-specific instructions (`.mjolnr/instructions.md`), skills, and persona defaults.
* **Nested Session List**:
  * Sessions belonging to the project are nested directly underneath.
  * Each session item shows title / task summary, unread activity dot, and rollup status badge (`in-progress`, `waiting-approval`, `verified`).
  * Clicking a session switches the main conversation canvas to that session.
  * Progressive disclosure: Shows the latest 5 sessions with a "Show more..." expander.

---

### 3.3 Nested Worktrees & Fleet Projection
Under an active session:
* **Local Subagents**: Worktree branches (`mjolnr/sub-xxx`) displayed with live execution status orb and branch pill.
* **Cloud Agents (Google Jules)**: Remote cloud runs displayed with cloud icon (`☁️`), live activity summary (e.g. `Google Jules: Exploring codebase`), and plan review badge (`Review Plan ↗`).

---

### 3.4 Recents (Collapsible Archive)
* Collapsible group at the bottom of the project list.
* Stores inactive or historical sessions from older projects, searchable via the `⌘K` palette.

---

### 3.5 Footer
* **User Profile**: Current operator avatar, name, and connected provider credentials status.
* **Voice / Dictation (`🎙️`)**: Hands-free voice input toggle.
* **Governance / Soul (`⚙️`)**: 1-click access to `SOUL.md`, route defaults, and policy tier picker.

---

## 4. Architectural Data Model Mapping

In the shared Rust runtime and client-bridge DTOs (`desktop/src/lib/runtime/contract.ts`):

```typescript
export interface ClientProject {
  id: string;
  name: string;
  rootPath: string;
  associatedPaths: string[];
  isPinned: boolean;
  sessions: ClientSessionSummary[];
  worktrees: ClientWorktreeSummary[];
  cloudAgents: ClientCloudAgentSummary[];
}

export interface ClientNavigationState {
  activeProjectId?: string;
  activeSessionId?: string;
  projects: ClientProject[];
  recents: ClientSessionSummary[];
  globalCounters: {
    pendingPullRequests: number;
    activePreviews: number;
    scheduledTasks: number;
    enabledPlugins: number;
  };
}
```

---

## 5. Summary of Benefits

1. **Zero Cognitive Disorientation**: Users always know *which project* a conversation and its worktrees belong to.
2. **First-Class Cloud Fleet Support**: Google Jules and local subagents live side-by-side as nested execution children of the project.
3. **Parity with Best-in-Class Developer Workspaces**: Delivers the clean, multi-project ergonomics of Codex and Orca while strictly enforcing mjolnr's governed execution and provenance model.
