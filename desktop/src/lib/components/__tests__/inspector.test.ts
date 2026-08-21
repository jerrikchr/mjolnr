// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import InspectorPane from '../inspector/InspectorPane.svelte';
import { clientStore, resetClientStoreForTests } from '$lib/runtime/client.svelte';
import type { ClientSnapshot } from '$lib/runtime/contract';
import { NO_PROJECT_REPOSITORY, NO_REVIEW_THREADS } from '$lib/runtime/contract';

const baseSnapshot: ClientSnapshot = {
  revision: 1,
  session: '0190d5f0-test-session',
  provider: 'anthropic',
  model: 'claude-3-5-sonnet',
  workspaceRoot: '/test/root',
  policy: 'ask',
  runActive: false,
  usage: { inputTokens: 1250, outputTokens: 430 },
  budget: { providerTurns: 3, maxProviderTurns: 25, toolCalls: 2, maxToolCalls: 100 },
  messages: [],
  messagesOmitted: 0,
  recovery: { state: 'clean' },
  models: [{ provider: 'anthropic', model: 'claude-3-5-sonnet', displayName: 'Claude 3.5 Sonnet' }],
  personas: [],
  souls: [],
  routes: [],
  accounts: [],
  sessions: [
    {
      id: '0190d5f0-test-session',
      title: 'Active Session 1',
      projectRoot: '/test/root',
      status: 'active',
      rollupStatus: 'running',
      provider: 'anthropic',
      model: 'claude-3-5-sonnet',
      updatedAt: '2026-07-29',
      eventCount: 10,
      leased: true
    }
  ],
  repository: NO_PROJECT_REPOSITORY,
  reviewThreads: NO_REVIEW_THREADS
};

describe('InspectorPane Component', () => {
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
  });

  it('renders session meta, token telemetry, and fleet summary', () => {
    const { getByText, getByTestId } = render(InspectorPane);

    expect(getByText('Inspector & Telemetry')).toBeDefined();
    expect(getByText('0190d5f0-test-session')).toBeDefined();
    expect(getByText('1,250')).toBeDefined();
    expect(getByText('430')).toBeDefined();
    expect(getByText('Active Session 1')).toBeDefined();
    expect(getByTestId('context-diagnostics').textContent).toContain(
      'No context diagnostics reported.'
    );
  });
});
