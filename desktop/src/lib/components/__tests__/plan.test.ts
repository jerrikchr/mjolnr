// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, cleanup } from '@testing-library/svelte';
import PlanSurface from '../plan/PlanSurface.svelte';
import { clientStore, resetClientStoreForTests } from '$lib/runtime/client.svelte';
import type { ClientSnapshot, ClientPlanWorkflow } from '$lib/runtime/contract';
import { NO_PROJECT_REPOSITORY, NO_REVIEW_THREADS } from '$lib/runtime/contract';

const baseSnapshot: ClientSnapshot = {
  revision: 1,
  session: '0190d5f0-test-session',
  provider: 'anthropic',
  model: 'claude-3-5-sonnet',
  workspaceRoot: '/test/root',
  policy: 'ask',
  runActive: false,
  usage: { inputTokens: 100, outputTokens: 50 },
  budget: { providerTurns: 1, maxProviderTurns: 25, toolCalls: 0, maxToolCalls: 100 },
  messages: [],
  messagesOmitted: 0,
  recovery: { state: 'clean' },
  models: [{ provider: 'anthropic', model: 'claude-3-5-sonnet', displayName: 'Claude 3.5 Sonnet' }],
  personas: [],
  souls: [],
  routes: [],
  accounts: [],
  sessions: [],
  repository: NO_PROJECT_REPOSITORY,
  reviewThreads: NO_REVIEW_THREADS
};

describe('PlanSurface Component', () => {
  beforeEach(() => {
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      }
    );
    resetClientStoreForTests();
    clientStore.snapshot = { ...baseSnapshot };
  });

  afterEach(() => {
    resetClientStoreForTests();
    cleanup();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('renders empty state when no plan exists', () => {
    const { getByText } = render(PlanSurface);
    expect(getByText('No Active Structured Plan')).toBeDefined();
  });

  it('renders the real unit-enum Idle wire shape without throwing', () => {
    clientStore.snapshot = {
      ...baseSnapshot,
      plan: {
        planId: 'plan-idle',
        stage: 'Idle',
        proposals: [],
        reviews: [],
        approvals: [],
        handoffs: []
      }
    };

    const { getByText } = render(PlanSurface);
    expect(getByText('No Active Structured Plan')).toBeDefined();
  });

  it('renders proposed plan title, revision, and steps', () => {
    const planWorkflow: ClientPlanWorkflow = {
      planId: 'plan-123',
      activeRevision: 1,
      stage: {
        Proposed: {
          proposal: {
            planId: 'plan-123',
            revisionId: 1,
            title: 'Add Authentication Module',
            summary: 'Implement secure login endpoint with JWT',
            steps: [
              { index: 1, title: 'Create DB Schema', description: 'Migration script for users' },
              { index: 2, title: 'Add Auth Handler', description: 'JWT sign and verify' }
            ],
            proposedAt: '2026-07-29T10:00:00Z'
          }
        }
      },
      proposals: [],
      reviews: [],
      approvals: [],
      handoffs: []
    };

    clientStore.snapshot = { ...baseSnapshot, plan: planWorkflow };

    const { getByText } = render(PlanSurface);

    expect(getByText('Add Authentication Module')).toBeDefined();
    expect(getByText('Implement secure login endpoint with JWT')).toBeDefined();
    expect(getByText('Create DB Schema')).toBeDefined();
    expect(getByText('Add Auth Handler')).toBeDefined();
    expect(getByText('Proposed — Approval Required')).toBeDefined();
  });

  it('renders clarification question pending and allows submitting answer', async () => {
    const dispatchSpy = vi.spyOn(clientStore, 'dispatch');

    const planWorkflow: ClientPlanWorkflow = {
      planId: 'plan-456',
      activeRevision: 1,
      stage: {
        QuestionPending: {
          question: {
            id: 'q-1',
            prompt: 'Which authentication strategy should we adopt?',
            options: ['OAuth2', 'JWT', 'Session Cookie'],
            isMultiSelect: false,
            createdAt: '2026-07-29T10:00:00Z'
          }
        }
      },
      proposals: [],
      reviews: [],
      approvals: [],
      handoffs: []
    };

    clientStore.snapshot = { ...baseSnapshot, plan: planWorkflow };

    const { getByText } = render(PlanSurface);

    expect(getByText('Clarification Obligation')).toBeDefined();
    expect(getByText('Which authentication strategy should we adopt?')).toBeDefined();

    const optionPill = getByText('OAuth2');
    await fireEvent.click(optionPill);

    const submitBtn = getByText('Submit Answer');
    await fireEvent.click(submitBtn);

    expect(dispatchSpy).toHaveBeenCalledWith({
      type: 'answerPlanQuestion',
      planId: 'plan-456',
      questionId: 'q-1',
      selectedOptions: ['OAuth2'],
      freeformText: undefined
    });
  });

  it('dispatches approvePlan when human governance controls are clicked', async () => {
    const dispatchSpy = vi.spyOn(clientStore, 'dispatch');

    const planWorkflow: ClientPlanWorkflow = {
      planId: 'plan-789',
      activeRevision: 1,
      stage: {
        Proposed: {
          proposal: {
            planId: 'plan-789',
            revisionId: 1,
            title: 'Refactor Database Layer',
            summary: 'Migrate sqlite connection pooling',
            steps: [{ index: 1, title: 'Pool setup', description: 'Use deadpool' }],
            proposedAt: '2026-07-29T10:00:00Z'
          }
        }
      },
      proposals: [],
      reviews: [],
      approvals: [],
      handoffs: []
    };

    clientStore.snapshot = { ...baseSnapshot, plan: planWorkflow };

    const { getByText } = render(PlanSurface);

    const approveBtn = getByText('Approve Plan');
    await fireEvent.click(approveBtn);

    expect(dispatchSpy).toHaveBeenCalledWith({
      type: 'approvePlan',
      planId: 'plan-789',
      revision: 1,
      decision: 'approve',
      note: undefined
    });
  });

  it('detects superseded/stale revision and hides governance action controls', () => {
    const planWorkflow: ClientPlanWorkflow = {
      planId: 'plan-999',
      activeRevision: 2, // Active is v2
      stage: {
        Proposed: {
          proposal: {
            planId: 'plan-999',
            revisionId: 1, // Proposal is stale v1
            title: 'Stale Plan',
            summary: 'Superseded proposal',
            steps: [],
            proposedAt: '2026-07-29T10:00:00Z'
          }
        }
      },
      proposals: [],
      reviews: [],
      approvals: [],
      handoffs: []
    };

    clientStore.snapshot = { ...baseSnapshot, plan: planWorkflow };

    const { getByText, queryByText } = render(PlanSurface);

    expect(getByText('SUPERSEDED REVISION')).toBeDefined();
    expect(queryByText('Approve Plan')).toBeNull();
  });
});
