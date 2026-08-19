// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, cleanup } from '@testing-library/svelte';
import OnboardingWizard from '../onboarding/OnboardingWizard.svelte';
import { clientStore, resetClientStoreForTests } from '$lib/runtime/client.svelte';
import type { ClientOnboardingPreview, ClientSnapshot } from '$lib/runtime/contract';
import { NO_PROJECT_REPOSITORY, NO_REVIEW_THREADS } from '$lib/runtime/contract';

const snapshot: ClientSnapshot = {
  revision: 1,
  policy: 'ask',
  runActive: false,
  usage: { inputTokens: 0, outputTokens: 0 },
  budget: { providerTurns: 0, maxProviderTurns: 25, toolCalls: 0, maxToolCalls: 100 },
  messages: [],
  messagesOmitted: 0,
  recovery: { state: 'clean' },
  models: [],
  personas: [],
  souls: [],
  routes: [{ name: 'default', roles: ['default'], provider: 'openai-codex', model: 'gpt-5.4' }],
  council: null,
  accounts: [{ provider: 'openai-codex', state: 'connected' }],
  sessions: [],
  repository: NO_PROJECT_REPOSITORY,
  reviewThreads: NO_REVIEW_THREADS
};

const preview: ClientOnboardingPreview = {
  root: '/tmp/project',
  files: [
    { path: '.smed/SOUL.md', action: 'write' },
    { path: '.smed/USER.md', action: 'preserve' }
  ]
};

describe('Onboarding wizard', () => {
  beforeEach(() => {
    resetClientStoreForTests();
    clientStore.connected = true;
    clientStore.snapshot = snapshot;
    vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    vi.spyOn(clientStore, 'onboardingPreview').mockResolvedValue(preview);
    vi.spyOn(clientStore, 'onboardingWrite').mockResolvedValue(preview);
    vi.spyOn(clientStore, 'setPolicy').mockResolvedValue(null);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('walks the owner through a truthful five-step setup and writes only the previewed files', async () => {
    const { getByLabelText, getByRole, getByTestId, getByText } = render(OnboardingWizard);

    await fireEvent.input(getByLabelText('Repository path'), { target: { value: '/tmp/project' } });
    await fireEvent.click(getByRole('button', { name: /Continue to Provider access/ }));
    expect(getByText(/never entered into this window/)).toBeDefined();

    await fireEvent.click(getByRole('button', { name: /Continue to Identity/ }));
    await fireEvent.click(getByRole('button', { name: /Generate deterministic draft/ }));
    expect((getByLabelText('SOUL.md preview') as HTMLTextAreaElement).value).toContain('Fail closed');

    await fireEvent.click(getByRole('button', { name: /Continue to Routes & council/ }));
    expect(getByText(/The council is advisory evidence for human disposition/)).toBeDefined();
    await fireEvent.click(getByRole('button', { name: /Continue to Policy gate/ }));
    expect(getByTestId('onboarding-preview')).toBeDefined();

    await fireEvent.click(getByRole('button', { name: /Finish setup & launch/ }));
    expect(clientStore.onboardingWrite).toHaveBeenCalledWith(
      expect.objectContaining({ root: '/tmp/project', soul: expect.stringContaining('Fail closed') })
    );
    expect(clientStore.setPolicy).toHaveBeenCalledWith('ask');
    expect(getByTestId('onboarding-complete')).toBeDefined();
  });
});
