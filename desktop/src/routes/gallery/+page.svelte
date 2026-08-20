<script lang="ts">
  import { goto } from '$app/navigation';
  import {
    ArrowLeft01Icon,
    CheckmarkCircle02Icon,
    InformationCircleIcon,
    SearchIcon,
    Alert02Icon
  } from '@hugeicons/core-free-icons';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import * as Alert from '$lib/components/ui/alert';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import * as Field from '$lib/components/ui/field';
  import { Input } from '$lib/components/ui/input';
  import * as Tabs from '$lib/components/ui/tabs';
  import { Textarea } from '$lib/components/ui/textarea';

  let exampleInput = $state('');
  let exampleMessage = $state('');
</script>

<main class="h-full overflow-auto">
  <div class="mx-auto flex w-full max-w-6xl flex-col gap-8 p-8">
    <header class="flex items-start justify-between gap-4">
      <div class="flex flex-col gap-2">
        <Badge variant="secondary">Nova · Obsidian Cyan · HugeIcons · Inter/JetBrains Mono</Badge>
        <h1 class="text-2xl font-semibold">mjolnr component gallery</h1>
        <p class="max-w-2xl text-sm text-muted-foreground">
          This route uses the same generated shadcn-svelte components as the desktop workspace. It is a visual smoke surface, not a second design system.
        </p>
      </div>
      <Button variant="outline" onclick={() => void goto('/')}>
        <HugeiconsIcon icon={ArrowLeft01Icon} strokeWidth={2} data-icon="inline-start" />
        Back to workspace
      </Button>
    </header>

    <Tabs.Root value="actions">
      <Tabs.List>
        <Tabs.Trigger value="actions">Actions</Tabs.Trigger>
        <Tabs.Trigger value="forms">Forms</Tabs.Trigger>
        <Tabs.Trigger value="feedback">Feedback</Tabs.Trigger>
      </Tabs.List>

      <Tabs.Content value="actions">
        <Card.Root>
          <Card.Header>
            <Card.Title>Governed actions</Card.Title>
            <Card.Description>Standard variants cover ordinary, secondary, and destructive decisions.</Card.Description>
          </Card.Header>
          <Card.Content class="flex flex-wrap gap-3">
            <Button>
              <HugeiconsIcon icon={CheckmarkCircle02Icon} strokeWidth={2} data-icon="inline-start" />
              Approve once
            </Button>
            <Button variant="secondary">Approve exact for session</Button>
            <Button variant="outline">Inspect details</Button>
            <Button variant="destructive">Deny</Button>
            <Button variant="ghost" size="icon" aria-label="Search">
              <HugeiconsIcon icon={SearchIcon} strokeWidth={2} />
            </Button>
          </Card.Content>
        </Card.Root>
      </Tabs.Content>

      <Tabs.Content value="forms">
        <Card.Root>
          <Card.Header>
            <Card.Title>Session inputs</Card.Title>
            <Card.Description>Field composition supplies labels, descriptions, and validation structure.</Card.Description>
          </Card.Header>
          <Card.Content>
            <Field.Group>
              <Field.Field>
                <Field.Label for="gallery-project">Project root</Field.Label>
                <Input id="gallery-project" placeholder="/path/to/project" bind:value={exampleInput} />
                <Field.Description>Passed to the runtime only when Open workspace is invoked.</Field.Description>
              </Field.Field>
              <Field.Field>
                <Field.Label for="gallery-message">Instruction</Field.Label>
                <Textarea id="gallery-message" rows={4} bind:value={exampleMessage} />
              </Field.Field>
            </Field.Group>
          </Card.Content>
        </Card.Root>
      </Tabs.Content>

      <Tabs.Content value="feedback">
        <div class="grid gap-4 md:grid-cols-2">
          <Alert.Root>
            <HugeiconsIcon icon={InformationCircleIcon} strokeWidth={2} />
            <Alert.Title>Runtime information</Alert.Title>
            <Alert.Description>Informational state uses the default semantic treatment.</Alert.Description>
          </Alert.Root>
          <Alert.Root variant="destructive">
            <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
            <Alert.Title>Governance refusal</Alert.Title>
            <Alert.Description>Destructive treatment is reserved for failures, refusal, and unsafe continuation.</Alert.Description>
          </Alert.Root>
        </div>
      </Tabs.Content>
    </Tabs.Root>
  </div>
</main>
