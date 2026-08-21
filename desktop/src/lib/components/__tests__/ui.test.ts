// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render } from '@testing-library/svelte';
import ButtonHarness from './ui/ButtonHarness.svelte';
import TabsHarness from './ui/TabsHarness.svelte';
import { MjolnrClient } from '$lib/runtime/client.svelte';
import type { ClientSnapshot } from '$lib/runtime/contract';
import { NO_PROJECT_REPOSITORY, NO_REVIEW_THREADS } from '$lib/runtime/contract';

const sampleSnapshot: ClientSnapshot = {
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
  sessions: [
    {
      id: '0190d5f0-test-session',
      title: 'Session 1',
      projectRoot: '/test/root',
      status: 'active',
      rollupStatus: 'running',
      provider: 'anthropic',
      model: 'claude-3-5-sonnet',
      updatedAt: '2026-07-28',
      eventCount: 5,
      leased: true
    }
  ],
  repository: NO_PROJECT_REPOSITORY,
  reviewThreads: NO_REVIEW_THREADS
};

describe('shadcn-svelte component integration', () => {
  it('uses the generated button variants without a custom wrapper', async () => {
    const onAction = vi.fn();
    const { getByRole } = render(ButtonHarness, { onAction });

    await fireEvent.click(getByRole('button', { name: 'Approve once' }));

    expect(onAction).toHaveBeenCalledOnce();
  });

  it('uses the generated accessible tabs primitive', async () => {
    const { getByRole, getByText } = render(TabsHarness);

    await fireEvent.click(getByRole('tab', { name: 'Verify' }));

    expect(getByText('Verification evidence')).toBeDefined();
  });
});

describe('Tauri IPC command dispatch', () => {
  beforeEach(() => {
    delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it('dispatches resumeSession payload over Tauri IPC', async () => {
    const invokeMock = vi.fn().mockResolvedValue(undefined);
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {
      invoke: invokeMock
    };

    const client = new MjolnrClient();
    client.handleUpdate({ type: 'snapshot', snapshot: sampleSnapshot });
    await client.resumeSession('0190d5f0-test-session');

    expect(invokeMock).toHaveBeenCalledWith(
      'dispatch_command',
      { command: { type: 'resumeSession', session: '0190d5f0-test-session' } },
      undefined
    );
  });

  it('dispatches resolveResume payload over Tauri IPC', async () => {
    const invokeMock = vi.fn().mockResolvedValue(undefined);
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {
      invoke: invokeMock
    };

    const client = new MjolnrClient();
    await client.resolveResume('compact');

    expect(invokeMock).toHaveBeenCalledWith(
      'dispatch_command',
      { command: { type: 'resolveResume', choice: 'compact' } },
      undefined
    );
  });

  it('clears a transient bridge refusal after a later command is accepted', async () => {
    const invokeMock = vi.fn().mockResolvedValue(undefined);
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {
      invoke: invokeMock
    };

    const client = new MjolnrClient();
    client.lastError = 'no session is open';

    await client.resolveResume('compact');

    expect(client.lastError).toBeNull();
  });
});
