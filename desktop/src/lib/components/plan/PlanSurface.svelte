<script lang="ts">
  import { clientStore } from '$lib/runtime/client.svelte';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import * as Empty from '$lib/components/ui/empty';
  import * as Field from '$lib/components/ui/field';
  import { ScrollArea } from '$lib/components/ui/scroll-area';
  import { Separator } from '$lib/components/ui/separator';
  import { Textarea } from '$lib/components/ui/textarea';
  import * as ToggleGroup from '$lib/components/ui/toggle-group';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import { FolderKanbanIcon } from '@hugeicons/core-free-icons';
  import type { ClientPlanWorkflow, ClientReviewVerdict } from '$lib/runtime/contract';

  let freeformAnswer = $state('');
  let selectedQuestionOptions = $state<string[]>([]);
  let approvalNote = $state('');

  let snap = $derived(clientStore.snapshot);
  let plan = $derived<ClientPlanWorkflow | undefined>(snap.plan);

  let stageType = $derived.by(() => {
    if (!plan) return 'empty';
    const stage = plan.stage;
    if (stage === 'Idle') return 'Idle';
    if ('QuestionPending' in stage) return 'QuestionPending';
    if ('Proposed' in stage) return 'Proposed';
    if ('Reviewed' in stage) return 'Reviewed';
    if ('Approved' in stage) return 'Approved';
    if ('IterateRequested' in stage) return 'IterateRequested';
    if ('Rejected' in stage) return 'Rejected';
    if ('Handoff' in stage) return 'Handoff';
    return 'Idle';
  });

  let currentProposal = $derived.by(() => {
    if (!plan) return undefined;
    const stage = plan.stage;
    if (stage === 'Idle') return plan.proposals.slice(-1)[0];
    if ('Proposed' in stage) return stage.Proposed.proposal;
    if ('Reviewed' in stage) return stage.Reviewed.proposal;
    if ('Approved' in stage) return stage.Approved.proposal;
    if ('IterateRequested' in stage) return stage.IterateRequested.proposal;
    if ('Rejected' in stage) return stage.Rejected.proposal;
    if ('Handoff' in stage) return stage.Handoff.proposal;
    return plan.proposals.slice(-1)[0];
  });

  let isStaleRevision = $derived.by(() => {
    if (!plan || !currentProposal || plan.activeRevision === undefined) return false;
    return currentProposal.revisionId !== plan.activeRevision;
  });

  function handleSingleOption(value: string) {
    selectedQuestionOptions = value ? [value] : [];
  }

  function handleMultipleOptions(values: string[]) {
    selectedQuestionOptions = values;
  }

  function handleAnswerQuestion(questionId: string) {
    if (!plan) return;
    void clientStore.dispatch({
      type: 'answerPlanQuestion',
      planId: plan.planId,
      questionId,
      selectedOptions: selectedQuestionOptions,
      freeformText: freeformAnswer.trim() ? freeformAnswer.trim() : undefined
    });
    selectedQuestionOptions = [];
    freeformAnswer = '';
  }

  function handleApprovePlan(verdict: ClientReviewVerdict) {
    if (!plan || !currentProposal) return;
    void clientStore.dispatch({
      type: 'approvePlan',
      planId: plan.planId,
      revision: currentProposal.revisionId,
      decision: verdict,
      note: approvalNote.trim() ? approvalNote.trim() : undefined
    });
    approvalNote = '';
  }

  function stageVariant() {
    if (stageType === 'Rejected' || isStaleRevision) return 'destructive';
    if (stageType === 'Approved' || stageType === 'Handoff') return 'secondary';
    return 'outline';
  }

  function stageLabel() {
    if (stageType === 'QuestionPending') return 'Question pending';
    if (stageType === 'Proposed') return 'Proposed — Approval Required';
    if (stageType === 'Reviewed') return 'Advisory reviewed';
    if (stageType === 'Approved') return 'Approved';
    if (stageType === 'IterateRequested') return 'Iteration requested';
    if (stageType === 'Rejected') return 'Rejected';
    if (stageType === 'Handoff') return 'Handed off to execution';
    return stageType;
  }
</script>

<div class="flex h-full w-full flex-col gap-4 overflow-y-auto p-4" data-testid="plan-surface">
  {#if !plan || stageType === 'empty' || stageType === 'Idle'}
    <Empty.Root class="border border-dashed">
      <Empty.Header>
        <Empty.Media variant="icon">
          <HugeiconsIcon icon={FolderKanbanIcon} strokeWidth={2} />
        </Empty.Media>
        <Empty.Title>No Active Structured Plan</Empty.Title>
        <Empty.Description>
          Plans proposed by the model will render here with step-level governance telemetry and human approval controls.
        </Empty.Description>
      </Empty.Header>
    </Empty.Root>
  {:else}
    <header class="flex flex-col gap-2">
      <div class="flex flex-col justify-between gap-3 sm:flex-row sm:items-center">
        <h1 class="text-2xl font-semibold tracking-tight">{currentProposal?.title || 'Execution Plan'}</h1>
        <div class="flex flex-wrap items-center gap-2">
          {#if currentProposal}
            <Badge variant="outline">v{currentProposal.revisionId}</Badge>
          {/if}
          <Badge variant={stageVariant()}>{stageLabel()}</Badge>
          {#if isStaleRevision}
            <Badge variant="destructive">SUPERSEDED REVISION</Badge>
          {/if}
        </div>
      </div>
      {#if currentProposal?.summary}
        <p class="text-muted-foreground text-sm">{currentProposal.summary}</p>
      {/if}
    </header>

    {#if typeof plan.stage !== 'string' && 'QuestionPending' in plan.stage}
      {@const question = plan.stage.QuestionPending.question}
      <Card.Root>
        <Card.Header>
          <Card.Title>Clarification Obligation</Card.Title>
          <Card.Description>{question.prompt}</Card.Description>
        </Card.Header>
        <Card.Content>
          <Field.Group>
            {#if question.options.length > 0}
              <Field.Field>
                <Field.Label>Available answers</Field.Label>
                {#if question.isMultiSelect}
                  <ToggleGroup.Root
                    type="multiple"
                    variant="outline"
                    class="flex-wrap"
                    value={selectedQuestionOptions}
                    onValueChange={handleMultipleOptions}
                  >
                    {#each question.options as option}
                      <ToggleGroup.Item value={option}>{option}</ToggleGroup.Item>
                    {/each}
                  </ToggleGroup.Root>
                {:else}
                  <ToggleGroup.Root
                    type="single"
                    variant="outline"
                    class="flex-wrap"
                    value={selectedQuestionOptions[0] ?? ''}
                    onValueChange={handleSingleOption}
                  >
                    {#each question.options as option}
                      <ToggleGroup.Item value={option}>{option}</ToggleGroup.Item>
                    {/each}
                  </ToggleGroup.Root>
                {/if}
              </Field.Field>
            {/if}
            <Field.Field>
              <Field.Label for="plan-clarification-note">Additional Details (Optional)</Field.Label>
              <Textarea
                id="plan-clarification-note"
                placeholder="Type clarification notes..."
                bind:value={freeformAnswer}
                rows={2}
              />
            </Field.Field>
          </Field.Group>
        </Card.Content>
        <Card.Footer class="justify-end">
          <Button onclick={() => handleAnswerQuestion(question.id)}>Submit Answer</Button>
        </Card.Footer>
      </Card.Root>
    {/if}

    {#if currentProposal?.steps && currentProposal.steps.length > 0}
      <Card.Root>
        <Card.Header>
          <Card.Title>Plan Execution Steps</Card.Title>
          <Card.Description>{currentProposal.steps.length} ordered step{currentProposal.steps.length === 1 ? '' : 's'}</Card.Description>
        </Card.Header>
        <Card.Content>
          <ScrollArea class="h-[300px]">
            <ol class="flex flex-col gap-3 pr-4">
              {#each currentProposal.steps as step}
                <li class="flex items-start gap-3">
                  <Badge variant="outline">Step {step.index}</Badge>
                  <div class="flex flex-col gap-1">
                    <h3 class="text-sm font-medium">{step.title}</h3>
                    <p class="text-muted-foreground text-sm">{step.description}</p>
                  </div>
                </li>
              {/each}
            </ol>
          </ScrollArea>
        </Card.Content>
      </Card.Root>
    {/if}

    {#if typeof plan.stage !== 'string' && 'Reviewed' in plan.stage}
      {@const reviews = plan.stage.Reviewed.reviews}
      <Card.Root>
        <Card.Header>
          <Card.Title>Advisory Council & Model Reviews</Card.Title>
          <Card.Description>Advisory output only; human approval remains authoritative.</Card.Description>
        </Card.Header>
        <Card.Content class="flex flex-col gap-3">
          {#each reviews as review, index}
            {#if index > 0}<Separator />{/if}
            <div class="flex flex-col gap-2">
              <div class="flex items-center justify-between gap-3">
                <h3 class="text-sm font-medium">{review.reviewer}</h3>
                <Badge variant={review.verdict === 'reject' ? 'destructive' : review.verdict === 'approve' ? 'secondary' : 'outline'}>
                  {review.verdict}
                </Badge>
              </div>
              {#if review.feedback}
                <p class="text-muted-foreground text-sm">{review.feedback}</p>
              {/if}
            </div>
          {/each}
        </Card.Content>
      </Card.Root>
    {/if}

    {#if (stageType === 'Proposed' || stageType === 'Reviewed') && !isStaleRevision}
      <Card.Root>
        <Card.Header>
          <Card.Title>Human Governance Controls</Card.Title>
          <Card.Description>Approve this exact revision, request another iteration, or reject it.</Card.Description>
        </Card.Header>
        <Card.Content>
          <Field.Field>
            <Field.Label for="plan-governance-note">Approval / Feedback Note</Field.Label>
            <Textarea
              id="plan-governance-note"
              placeholder="Optional notes for model iteration or audit record..."
              bind:value={approvalNote}
              rows={2}
            />
          </Field.Field>
        </Card.Content>
        <Card.Footer class="flex-wrap gap-2">
          <Button onclick={() => handleApprovePlan('approve')}>Approve Plan</Button>
          <Button variant="secondary" onclick={() => handleApprovePlan('iterate')}>Request Iteration</Button>
          <Button variant="destructive" onclick={() => handleApprovePlan('reject')}>Reject Plan</Button>
        </Card.Footer>
      </Card.Root>
    {/if}
  {/if}
</div>
