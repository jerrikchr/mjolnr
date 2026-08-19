<script lang="ts">
  import { goto } from '$app/navigation';
  import { open } from '@tauri-apps/plugin-dialog';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import {
    ArrowLeft01Icon,
    ArrowRight01Icon,
    CheckmarkCircle02Icon,
    FolderOpenIcon,
    Key01Icon,
    SparklesIcon
  } from '@hugeicons/core-free-icons';
  import AppEmblem from '$lib/components/chrome/AppEmblem.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import * as Field from '$lib/components/ui/field';
  import { Input } from '$lib/components/ui/input';
  import { Textarea } from '$lib/components/ui/textarea';
  import { clientStore, type ClientRefusal } from '$lib/runtime/client.svelte';
  import type {
    ClientOnboardingDraft,
    ClientOnboardingPreview,
    ClientPolicy
  } from '$lib/runtime/contract';

  type Step = 1 | 2 | 3 | 4 | 5;
  type Stack = 'rust' | 'typescript' | 'python' | 'general';
  type Priority = 'safety' | 'architecture' | 'speed';
  type CouncilDepth = 'quick' | 'deep';

  const steps: Array<{ id: Step; label: string }> = [
    { id: 1, label: 'Workspace' },
    { id: 2, label: 'Provider access' },
    { id: 3, label: 'Identity' },
    { id: 4, label: 'Routes & council' },
    { id: 5, label: 'Policy gate' }
  ];
  const policies: Array<{ value: ClientPolicy; label: string; detail: string }> = [
    { value: 'read-only', label: 'Read-only', detail: 'Inspect state without commands or mutations.' },
    { value: 'ask', label: 'Ask', detail: 'Require human approval before a side effect.' },
    { value: 'workspace-write', label: 'Workspace write', detail: 'Allow declared workspace writes; keep other effects gated.' }
  ];

  const starterSoul = `# SOUL.md — smed's identity

You are smed, a local-first, governed coding harness. Be deliberate and honest:
say what you did and did not do, and seek approval before any side effect.
`;

  let currentStep = $state<Step>(1);
  let projectPath = $state('');
  let projectRefusal = $state<ClientRefusal | null>(null);
  let stack = $state<Stack>('rust');
  let priority = $state<Priority>('safety');
  let councilDepth = $state<CouncilDepth>('quick');
  let policy = $state<ClientPolicy>('ask');
  let soulText = $state(starterSoul);
  let userProfile = $state('');
  let preview = $state<ClientOnboardingPreview | null>(null);
  let previewRefusal = $state<ClientRefusal | null>(null);
  let writeRefusal = $state<ClientRefusal | null>(null);
  let writing = $state(false);
  let completed = $state(false);

  let snap = $derived(clientStore.snapshot);
  let accounts = $derived(snap.accounts);
  let routes = $derived(snap.routes);
  let connectedCount = $derived(accounts.filter((account) => account.state === 'connected').length);
  let modelsAvailable = $derived(snap.models.length);

  function refreshProviders() {
    void clientStore.dispatch({ type: 'requestSnapshot' });
  }

  function selectedClass(selected: boolean): string {
    return selected
      ? 'border-accent-bright bg-accent-muted text-foreground'
      : 'border-border bg-card text-muted-foreground hover:border-accent-bright/60';
  }

  function accountDetail(state: string): string {
    if (state === 'connected') return 'connected';
    if (state === 'needsReauth') return 'needs re-auth';
    if (state === 'unavailable') return 'unavailable';
    return 'not connected';
  }

  function generateSoul() {
    const stackLine: Record<Stack, string> = {
      rust: 'Prefer typed errors, bounded resources, and zero panics outside tests.',
      typescript: 'Prefer explicit interfaces, accessible states, and small reversible changes.',
      python: 'Prefer deterministic environments, typed boundaries, and reproducible tests.',
      general: 'Prefer clear boundaries, explicit assumptions, and evidence-backed completion.'
    };
    const priorityLine: Record<Priority, string> = {
      safety: 'Fail closed when a path, credential, or side effect is ambiguous.',
      architecture: 'Keep modules focused and preserve dependency direction.',
      speed: 'Keep the diff narrow and verify the smallest useful slice quickly.'
    };
    soulText = `${starterSoul.trimEnd()}\n- ${stackLine[stack]}\n- ${priorityLine[priority]}\n`;
  }

  function draft(): ClientOnboardingDraft {
    return {
      root: projectPath.trim(),
      soul: soulText,
      userProfile: userProfile.trim() ? `# USER.md — who smed works for\n\n${userProfile.trim()}\n` : undefined
    };
  }

  async function chooseWorkspace() {
    let root = projectPath.trim();
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      const picked = await open({ directory: true, multiple: false, title: 'Open workspace' });
      if (picked === null) return;
      root = picked;
    }
    projectPath = root;
    projectRefusal = await clientStore.dispatch({ type: 'openProject', root });
  }

  async function loadPreview() {
    previewRefusal = null;
    if (!projectPath.trim()) {
      previewRefusal = { code: 'PATH_OUTSIDE_WORKSPACE', message: 'Choose a workspace before previewing setup files.' };
      return;
    }
    const result = await clientStore.onboardingPreview(draft());
    if ('files' in result) preview = result;
    else previewRefusal = result;
  }

  async function nextStep() {
    if (currentStep === 1 && !projectPath.trim()) {
      projectRefusal = { code: 'PATH_OUTSIDE_WORKSPACE', message: 'Choose a workspace before continuing.' };
      return;
    }
    if (currentStep === 4) await loadPreview();
    if (currentStep < 5) currentStep = (currentStep + 1) as Step;
  }

  function previousStep() {
    if (currentStep > 1) currentStep = (currentStep - 1) as Step;
  }

  async function finishSetup() {
    writing = true;
    writeRefusal = null;
    const result = await clientStore.onboardingWrite(draft());
    if ('files' in result) {
      preview = result;
      const policyRefusal = await clientStore.setPolicy(policy);
      if (policyRefusal) writeRefusal = policyRefusal;
      else completed = true;
    } else {
      writeRefusal = result;
    }
    writing = false;
  }
</script>

<svelte:head>
  <title>smed · Guided setup</title>
</svelte:head>

<main class="min-h-screen bg-background px-4 py-8 text-foreground sm:px-8">
  <section class="mx-auto flex w-full max-w-4xl flex-col overflow-hidden rounded-xl border bg-card shadow-2xl" data-testid="onboarding-wizard">
    <header class="flex items-center justify-between border-b bg-muted/20 px-6 py-4">
      <div class="flex items-center gap-3">
        <AppEmblem size={28} />
        <div>
          <h1 class="font-semibold">Welcome to smed</h1>
          <p class="text-xs text-muted-foreground">Guided setup · about 3 minutes</p>
        </div>
      </div>
      <Button variant="ghost" size="sm" onclick={() => goto('/')}>Exit setup</Button>
    </header>

    <nav class="grid grid-cols-5 border-b" aria-label="Setup steps">
      {#each steps as step}
        <button
          type="button"
          class="border-b-2 px-2 py-3 text-xs transition-colors"
          class:border-accent-bright={currentStep === step.id}
          class:text-accent-bright={currentStep === step.id}
          class:text-gov-verified={currentStep > step.id}
          aria-current={currentStep === step.id ? 'step' : undefined}
          onclick={() => step.id <= currentStep && (currentStep = step.id)}
        >
          <span class="mr-1 font-mono">{currentStep > step.id ? '✓' : step.id}</span>{step.label}
        </button>
      {/each}
    </nav>

    <div class="min-h-[30rem] px-6 py-8 sm:px-10">
      {#if completed}
        <div class="flex min-h-[28rem] flex-col items-center justify-center gap-5 text-center" data-testid="onboarding-complete">
          <HugeiconsIcon icon={CheckmarkCircle02Icon} class="size-12 text-gov-verified" strokeWidth={2} />
          <div>
            <h2 class="text-xl font-semibold">Setup is ready</h2>
            <p class="mt-2 max-w-lg text-sm text-muted-foreground">
              Missing setup files were written under <code class="font-mono">.smed/</code>; existing files were preserved.
              The next step is still yours: a new project can begin the PRD interview, while an existing project can begin bounded discovery.
            </p>
          </div>
          <Button onclick={() => goto('/')}>Launch workspace</Button>
        </div>
      {:else if currentStep === 1}
        <div class="space-y-6">
          <div>
            <h2 class="text-xl font-semibold">Select your target workspace</h2>
            <p class="mt-2 max-w-2xl text-sm text-muted-foreground">
              smed works locally on a directory you choose. The wizard only offers to create missing, diffable files under <code class="font-mono">.smed/</code>; it does not change git history or merge branches.
            </p>
          </div>
          <Card.Root>
            <Card.Header>
              <Card.Title class="flex items-center gap-2"><HugeiconsIcon icon={FolderOpenIcon} class="size-4" /> Workspace path</Card.Title>
              <Card.Description>Absolute paths are canonicalised and refused if they are not directories.</Card.Description>
            </Card.Header>
            <Card.Content>
              <Field.Group>
                <Field.Field>
                  <Field.Label for="onboarding-root">Repository path</Field.Label>
                  <div class="flex gap-2">
                    <Input id="onboarding-root" bind:value={projectPath} placeholder="/absolute/path/to/project" />
                    <Button variant="outline" class="shrink-0" onclick={chooseWorkspace}>Browse</Button>
                  </div>
                  {#if projectRefusal}
                    <Field.Error data-testid="onboarding-root-refusal">{projectRefusal.code} · {projectRefusal.message}</Field.Error>
                  {/if}
                </Field.Field>
              </Field.Group>
            </Card.Content>
          </Card.Root>
          <div class="grid gap-3 sm:grid-cols-4">
            {#each [['Git', 'Rust core'], ['Local-first', 'No cloud state'], ['Diffable', '.smed files'], ['Governed', 'Rust owns effects']] as check}
              <div class="rounded-md border bg-muted/10 px-3 py-2 text-xs"><div class="font-medium">{check[0]}</div><div class="mt-1 text-muted-foreground">{check[1]}</div></div>
            {/each}
          </div>
        </div>
      {:else if currentStep === 2}
        <div class="space-y-6">
          <div>
            <h2 class="text-xl font-semibold">Connect a provider</h2>
            <p class="mt-2 max-w-2xl text-sm text-muted-foreground">Credentials are captured in the terminal with echo disabled and stay in your owner-only files — they are never entered into this window or the transcript.</p>
          </div>

          <div class="rounded-md border border-accent-bright/30 bg-accent-muted/30 p-4 text-sm">
            <div class="flex items-start gap-3">
              <HugeiconsIcon icon={Key01Icon} class="mt-0.5 size-4 text-accent-bright" />
              <p>In a terminal, run the provider's owner-authenticated login once, then press "Check status". smed reads the result from your credential files.</p>
            </div>
            <code class="mt-3 block rounded bg-background/70 p-2 font-mono text-xs">smed auth login openai-codex   # also: openai, anthropic, gemini, openrouter, ollama</code>
            <Button variant="outline" size="sm" class="mt-3" onclick={refreshProviders}>Check status</Button>
          </div>

          <div class="grid gap-3 sm:grid-cols-2">
            {#each ['anthropic', 'openai-codex', 'gemini-cli', 'ollama'] as provider}
              {@const account = accounts.find((item) => item.provider === provider)}
              <Card.Root>
                <Card.Header class="pb-3"><Card.Title class="text-sm">{provider}</Card.Title><Card.Description>{accountDetail(account?.state ?? 'disconnected')}</Card.Description></Card.Header>
                <Card.Footer>{#if account?.state === 'connected'}<Badge variant="secondary">Connected</Badge>{:else}<Badge variant="outline">Not connected</Badge>{/if}</Card.Footer>
              </Card.Root>
            {/each}
          </div>

          <div class="rounded-md border border-border bg-muted/20 p-4 text-sm">
            {#if modelsAvailable > 0}
              <p class="text-gov-verified">{connectedCount} provider{connectedCount === 1 ? '' : 's'} connected · {modelsAvailable} model{modelsAvailable === 1 ? '' : 's'} ready. You'll pick the exact model when you start a session.</p>
            {:else}
              <p class="text-muted-foreground">No models reported yet — connect a provider above, then press "Check status".</p>
            {/if}
          </div>
        </div>
      {:else if currentStep === 3}
        <div class="space-y-6">
          <div>
            <h2 class="text-xl font-semibold">Shape your identity contract</h2>
            <p class="mt-2 max-w-2xl text-sm text-muted-foreground">Answer two small questions to draft inert plain text. You can edit it before writing <code class="font-mono">.smed/SOUL.md</code>.</p>
          </div>
          <Card.Root>
            <Card.Header><Card.Title>Project shape</Card.Title></Card.Header>
            <Card.Content class="grid gap-2 sm:grid-cols-4">
              {#each [['rust', 'Rust / systems'], ['typescript', 'TypeScript / web'], ['python', 'Python / ML'], ['general', 'General app']] as option}
                <button type="button" class={`rounded-md border px-3 py-3 text-left text-xs ${selectedClass(stack === option[0])}`} onclick={() => (stack = option[0] as Stack)}>{option[1]}</button>
              {/each}
            </Card.Content>
          </Card.Root>
          <Card.Root>
            <Card.Header><Card.Title>Primary priority</Card.Title></Card.Header>
            <Card.Content class="grid gap-2 sm:grid-cols-3">
              {#each [['safety', 'Safety & zero panics'], ['architecture', 'Architecture & modularity'], ['speed', 'Speed & iteration']] as option}
                <button type="button" class={`rounded-md border px-3 py-3 text-left text-xs ${selectedClass(priority === option[0])}`} onclick={() => (priority = option[0] as Priority)}>{option[1]}</button>
              {/each}
            </Card.Content>
          </Card.Root>
          <div class="flex justify-end"><Button variant="outline" size="sm" onclick={generateSoul}><HugeiconsIcon icon={SparklesIcon} data-icon="inline-start" /> Generate deterministic draft</Button></div>
          <Field.Field>
            <Field.Label for="onboarding-soul">SOUL.md preview</Field.Label>
            <Textarea id="onboarding-soul" class="min-h-36 font-mono text-xs" bind:value={soulText} />
          </Field.Field>
          <Field.Field>
            <Field.Label for="onboarding-user">Optional working profile</Field.Label>
            <Textarea id="onboarding-user" class="min-h-20" bind:value={userProfile} placeholder="How should smed work with you?" />
          </Field.Field>
        </div>
      {:else if currentStep === 4}
        <div class="space-y-6">
          <div>
            <h2 class="text-xl font-semibold">Review routes and advisory depth</h2>
            <p class="mt-2 max-w-2xl text-sm text-muted-foreground">Routes come from the runtime's deterministic configuration. The council is advisory evidence for human disposition, not a quorum or execution grant.</p>
          </div>
          <Card.Root>
            <Card.Header><Card.Title>Current model routes</Card.Title><Card.Description>Nothing here silently changes provider or model selection.</Card.Description></Card.Header>
            <Card.Content>
              {#if routes.length === 0}<p class="text-sm text-muted-foreground">No routes are configured yet. Connect a provider and run the CLI scaffold, then return to review them.</p>{:else}<div class="space-y-2">{#each routes as route}<div class="flex flex-wrap items-center justify-between gap-2 rounded border px-3 py-2 text-sm"><span>{route.name}</span><span class="font-mono text-xs text-muted-foreground">{route.provider} · {route.model}</span></div>{/each}</div>{/if}
            </Card.Content>
          </Card.Root>
          <div class="grid gap-3 sm:grid-cols-2">
            <button type="button" class={`rounded-md border p-4 text-left ${selectedClass(councilDepth === 'quick')}`} onclick={() => (councilDepth = 'quick')}><div class="font-medium">Quick read</div><p class="mt-1 text-xs text-muted-foreground">A bounded advisory review with a small evidence set.</p></button>
            <button type="button" class={`rounded-md border p-4 text-left ${selectedClass(councilDepth === 'deep')}`} onclick={() => (councilDepth = 'deep')}><div class="font-medium">Deep review</div><p class="mt-1 text-xs text-muted-foreground">More advisory positions and rounds; still requires human disposition.</p></button>
          </div>
        </div>
      {:else}
        <div class="space-y-6">
          <div>
            <h2 class="text-xl font-semibold">Set the default policy gate</h2>
            <p class="mt-2 max-w-2xl text-sm text-muted-foreground">This choice applies through the runtime's ordinary policy command. It does not grant the model authority, and no test-drive action is executed here.</p>
          </div>
          <div class="grid gap-3 sm:grid-cols-3">
            {#each policies as option}
              <button type="button" class={`rounded-md border p-4 text-left ${selectedClass(policy === option.value)}`} onclick={() => (policy = option.value)}><div class="font-medium">{option.label}</div><p class="mt-1 text-xs text-muted-foreground">{option.detail}</p></button>
            {/each}
          </div>
          <Card.Root>
            <Card.Header><Card.Title class="flex items-center gap-2"><HugeiconsIcon icon={Key01Icon} class="size-4 text-gov-approval" /> Approval gate test-drive</Card.Title><Card.Description>Preview only — no command, file mutation, or provider request runs.</Card.Description></Card.Header>
            <Card.Content class="flex items-center justify-between gap-3 rounded-md bg-muted/20 p-3 text-xs"><code class="font-mono">replace_file_content · /src/main.rs</code><Badge variant="outline">human approval remains required</Badge></Card.Content>
          </Card.Root>
          {#if preview}
            <Card.Root data-testid="onboarding-preview"><Card.Header><Card.Title>Write preview</Card.Title><Card.Description>Only missing files will be created; existing files stay untouched.</Card.Description></Card.Header><Card.Content class="space-y-2">{#each preview.files as file}<div class="flex justify-between gap-3 text-sm"><code class="font-mono text-xs">{file.path}</code><span class={file.action === 'write' ? 'text-gov-verified' : 'text-muted-foreground'}>{file.action === 'write' ? 'will write' : 'preserve existing'}</span></div>{/each}</Card.Content></Card.Root>
          {:else if previewRefusal}<p class="text-sm text-gov-refusal">{previewRefusal.code} · {previewRefusal.message}</p>{/if}
          {#if writeRefusal}<p class="text-sm text-gov-refusal" data-testid="onboarding-write-refusal">{writeRefusal.code} · {writeRefusal.message}</p>{/if}
        </div>
      {/if}
    </div>

    {#if !completed}
      <footer class="flex items-center justify-between border-t bg-muted/20 px-6 py-4">
        <Button variant="outline" disabled={currentStep === 1} onclick={previousStep}><HugeiconsIcon icon={ArrowLeft01Icon} data-icon="inline-start" /> Back</Button>
        {#if currentStep === 5}
          <Button disabled={writing || !preview} onclick={finishSetup}>{writing ? 'Writing setup…' : 'Finish setup & launch'}<HugeiconsIcon icon={CheckmarkCircle02Icon} data-icon="inline-end" /></Button>
        {:else}
          <Button disabled={currentStep === 1 && !projectPath.trim()} onclick={nextStep}>Continue to {steps[currentStep].label}<HugeiconsIcon icon={ArrowRight01Icon} data-icon="inline-end" /></Button>
        {/if}
      </footer>
    {/if}
  </section>
</main>
