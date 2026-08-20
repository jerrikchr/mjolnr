// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, cleanup } from '@testing-library/svelte';
import AttentionSurface from '../surfaces/AttentionSurface.svelte';
import ChangesSurface from '../surfaces/ChangesSurface.svelte';
import VerifySurface from '../surfaces/VerifySurface.svelte';
import { clientStore, resetClientStoreForTests } from '$lib/runtime/client.svelte';
import type { ClientSnapshot } from '$lib/runtime/contract';
import { NO_PROJECT_REPOSITORY, NO_REVIEW_THREADS } from '$lib/runtime/contract';

const baseSnapshot: ClientSnapshot = {
  revision: 7,
  session: '0190d5f0-test-session',
  provider: 'anthropic',
  model: 'claude-3-5-sonnet',
  workspaceRoot: '/test/root',
  policy: 'ask',
  runActive: false,
  usage: { inputTokens: 220, outputTokens: 80 },
  budget: { providerTurns: 2, maxProviderTurns: 25, toolCalls: 2, maxToolCalls: 100 },
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

/// One changed file and no read evidence, for the pair of cases that differ
/// only in whether a read was ever recorded.
function changesWithoutEvidence() {
  return {
    state: 'currentWorkingTree' as const,
    files: [
      {
        path: 'src/main.ts',
        status: 'modified' as const,
        hunks: [],
        content: 'text' as const,
        isLarge: false,
        isTruncated: false
      }
    ],
    readEvidence: [],
    captureDigest: 'abc123',
    captureSequence: 4,
    filesTruncated: false,
    undiffedUntracked: []
  };
}

/// A one-hunk diff whose line 2 exists on both sides, so a note can be pinned
/// to it.
function changesWithDiff() {
  return {
    ...changesWithoutEvidence(),
    files: [
      {
        path: 'src/main.ts',
        status: 'modified' as const,
        hunks: [
          {
            header: '@@ -1,3 +1,3 @@',
            oldStart: 1,
            oldLines: 3,
            newStart: 1,
            newLines: 3,
            lines: [
              { kind: 'unchanged' as const, content: 'one', oldLineNumber: 1, newLineNumber: 1 },
              { kind: 'added' as const, content: 'TWO', newLineNumber: 2 },
              { kind: 'removed' as const, content: 'two', oldLineNumber: 2 }
            ]
          }
        ],
        content: 'text' as const,
        isLarge: false,
        isTruncated: false
      }
    ]
  };
}

function reviewThread(overrides: { anchorStale?: boolean; status?: string }) {
  return {
    id: '0190d5f0-0000-7000-8000-000000000004',
    status: overrides.status ?? 'open',
    commentCount: 1,
    commentCountTruncated: false,
    trust: 'operatorControlled' as const,
    anchor: {
      path: 'src/main.ts',
      side: 'new' as const,
      line: 2,
      hunkHeader: '@@ -1,3 +1,3 @@',
      captureDigest: 'abc123',
      baseObjectId: 'def456'
    },
    anchorStale: overrides.anchorStale ?? false,
    comments: [
      {
        body: 'this line needs a comment',
        bodyTruncated: false,
        createdAt: '2026-07-30T12:00:00Z'
      }
    ]
  };
}

describe('Desktop surfaces stay inside explicit snapshot authority', () => {
  beforeEach(() => {
    resetClientStoreForTests();
  });

  afterEach(() => {
    resetClientStoreForTests();
    cleanup();
  });

  it('ChangesSurface renders exact change-sets and pending previews', () => {
    clientStore.snapshot = {
      ...baseSnapshot,
      pendingApproval: {
        id: 'approval-1',
        toolName: 'edit_file',
        tier: 'write',
        preview: 'edit src/main.ts'
      },
      changes: {
        state: 'proposed',
        files: [
          {
            path: 'src/main.ts',
            status: 'modified',
            hunks: [
              {
                header: '@@ -1,5 +1,6 @@',
                oldStart: 1,
                oldLines: 5,
                newStart: 1,
                newLines: 6,
                lines: [
                  { kind: 'unchanged', content: 'line 1' },
                  { kind: 'added', content: 'line 2' }
                ]
              }
            ],
            content: 'text',
            isLarge: false,
            isTruncated: false
          }
        ],
        readEvidence: [],
        captureDigest: 'abc123',
        captureSequence: 4,
        filesTruncated: false,
        undiffedUntracked: []
      }
    };

    const { getByText } = render(ChangesSurface);

    expect(getByText('Exact file changes and line-level review.')).toBeDefined();
    expect(getByText('edit src/main.ts')).toBeDefined();
    expect(getByText('src/main.ts')).toBeDefined();
    expect(getByText('@@ -1,5 +1,6 @@')).toBeDefined();
    expect(getByText('line 2')).toBeDefined();
    // The freshness marker, not a currency claim: the surface reports when
    // mjolnr last looked, never that this is the tree as it stands now.
    expect(getByText('captured at #4')).toBeDefined();
  });

  it('ChangesSurface says what it is not showing rather than implying completeness', () => {
    clientStore.snapshot = {
      ...baseSnapshot,
      changes: {
        state: 'currentWorkingTree',
        files: [
          {
            path: 'assets/logo.png',
            status: 'modified',
            hunks: [],
            content: 'binary',
            isLarge: false,
            isTruncated: false
          },
          {
            path: 'legacy/latin1.txt',
            status: 'modified',
            hunks: [],
            content: 'undecodable',
            isLarge: false,
            isTruncated: false
          }
        ],
        readEvidence: [],
        captureDigest: 'def456',
        captureSequence: 9,
        filesTruncated: true,
        undiffedUntracked: ['scratch/']
      }
    };

    const { getByText } = render(ChangesSurface);

    // A bounded set that cannot admit it was bounded reads as a complete one.
    expect(getByText('This change set is partial')).toBeDefined();
    expect(getByText('1 untracked path not diffed')).toBeDefined();
    expect(getByText('scratch/')).toBeDefined();

    // Binary and not-UTF-8 are two causes with two remedies, so they get two
    // sentences and two badges — never one vague "no diff available".
    expect(getByText('Not UTF-8')).toBeDefined();
    expect(
      getByText('git reports this file as binary and does not produce a text diff.')
    ).toBeDefined();
    expect(
      getByText(
        'git returned bytes that are not valid UTF-8. mjolnr will not guess at an encoding, so no content is shown.'
      )
    ).toBeDefined();
  });

  it('ChangesSurface renders the empty state without inventing a diff', () => {
    clientStore.snapshot = { ...baseSnapshot };

    const { getByText, queryByText } = render(ChangesSurface);

    expect(getByText('No governed change records yet')).toBeDefined();
    expect(getByText('No explicit file changes are present in the active snapshot.')).toBeDefined();
    expect(queryByText('@@ -1,5 +1,6 @@')).toBeNull();
  });

  it('ChangesSurface keeps rename provenance visible', () => {
    clientStore.snapshot = {
      ...baseSnapshot,
      changes: {
        state: 'proposed',
        files: [
          {
            path: 'src/new_name.ts',
            status: 'renamed',
            oldPath: 'src/old_name.ts',
            hunks: [],
            content: 'text',
            isLarge: false,
            isTruncated: false
          }
        ],
        readEvidence: [],
        captureDigest: 'ghi789',
        captureSequence: 1,
        filesTruncated: false,
        undiffedUntracked: []
      }
    };

    const { getByText } = render(ChangesSurface);

    expect(getByText('src/new_name.ts')).toBeDefined();
    expect(getByText('← src/old_name.ts')).toBeDefined();
    expect(getByText('renamed')).toBeDefined();
    // A file with no printable hunks says so explicitly, per §D3's bounded
    // representations — it does not render a fake empty hunk.
    expect(getByText('No printable diff hunks.')).toBeDefined();
  });

  it('ChangesSurface cites the tool event behind a read', () => {
    clientStore.snapshot = {
      ...baseSnapshot,
      changes: {
        ...changesWithoutEvidence(),
        readEvidence: [
          {
            path: 'src/main.ts',
            readRevision: 'ed37e032a112657771a683fcd55cef59eb3957eb',
            toolEventId: '019fb36a-5db6-70f2-9f60-623dc788aa94'
          }
        ]
      }
    };

    const { getByText } = render(ChangesSurface);
    expect(getByText('Read before edit evidence:')).toBeDefined();
    // The citation is the point: the id names a durable event a reader can
    // look up, not a placeholder.
    expect(getByText(/tool event: 019fb36a-5db6-70f2-9f60-623dc788aa94/)).toBeDefined();
  });

  it('ChangesSurface shows no evidence section when nothing recorded a read', () => {
    // An empty "Read before edit evidence:" heading would read as "mjolnr
    // looked and found none", which is a different claim from "nothing
    // recorded a read". The section is absent instead.
    clientStore.snapshot = { ...baseSnapshot, changes: changesWithoutEvidence() };

    const { queryByText } = render(ChangesSurface);
    expect(queryByText('Read before edit evidence:')).toBeNull();
  });

  it('ChangesSurface keeps a stale note on the line it was written against', async () => {
    // §D3: "stale anchors remain visible but cannot silently move to a
    // different line." The surface has to hold both halves — the note is on
    // screen, on its own line, and it says the diff has moved.
    clientStore.snapshot = {
      ...baseSnapshot,
      changes: changesWithDiff(),
      reviewThreads: {
        ...NO_REVIEW_THREADS,
        items: [reviewThread({ anchorStale: true })],
        total: 1
      }
    };

    const { getByText, getAllByTestId } = render(ChangesSurface);

    expect(getAllByTestId('review-thread')).toHaveLength(1);
    expect(getByText('Note on new line 2')).toBeDefined();
    expect(getByText('this line needs a comment')).toBeDefined();
    expect(getByText('Diff has moved since this note was written')).toBeDefined();
    // And the diff around it still renders: a stale note does not blank the
    // review it belongs to.
    expect(getByText('@@ -1,3 +1,3 @@')).toBeDefined();
  });

  it('ChangesSurface sends the selected notes as a message that claims nothing', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    clientStore.snapshot = {
      ...baseSnapshot,
      changes: changesWithDiff(),
      reviewThreads: {
        ...NO_REVIEW_THREADS,
        items: [reviewThread({})],
        total: 1
      }
    };

    const { getByLabelText, getByText } = render(ChangesSurface);

    // Nothing selected: the control is there but refuses to fire, because an
    // empty request asks mjolnr for nothing.
    const send = getByText(/Send 0 to mjolnr/).closest('button');
    expect(send?.disabled).toBe(true);

    await fireEvent.click(getByLabelText('Note on new line 2'));
    await fireEvent.click(getByText(/Send 1 to mjolnr/));

    expect(dispatch).toHaveBeenCalledWith({
      type: 'sendReviewNotes',
      threadIds: ['0190d5f0-0000-7000-8000-000000000004']
    });
    // The surface says what sending is. It must not read as an approval.
    expect(getByText(/It approves nothing and changes no policy/)).toBeDefined();
    dispatch.mockRestore();
  });

  it('ChangesSurface can add a human reply to an existing review thread', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue(null);
    clientStore.snapshot = {
      ...baseSnapshot,
      changes: changesWithDiff(),
      reviewThreads: {
        ...NO_REVIEW_THREADS,
        items: [reviewThread({})],
        total: 1
      }
    };

    const { getByRole, getByLabelText } = render(ChangesSurface);

    await fireEvent.click(getByRole('button', { name: 'Reply to review thread' }));
    await fireEvent.input(getByLabelText('Reply body'), {
      target: { value: 'Please update this before merging.' }
    });
    await fireEvent.click(getByRole('button', { name: 'Add reply' }));

    expect(dispatch).toHaveBeenCalledWith({
      type: 'addReviewComment',
      threadId: '0190d5f0-0000-7000-8000-000000000004',
      body: 'Please update this before merging.'
    });
    dispatch.mockRestore();
  });

  it('ChangesSurface shows a refused note at the line it belongs to', async () => {
    const dispatch = vi.spyOn(clientStore, 'dispatch').mockResolvedValue({
      code: 'WORKSPACE_STALE_DIFF',
      message: 'This note was written against diff revision abc123'
    });
    clientStore.snapshot = { ...baseSnapshot, changes: changesWithDiff() };

    const { getByLabelText, getByText, getByRole } = render(ChangesSurface);

    await fireEvent.click(
      getByLabelText('Add a review note on src/main.ts line 2 (new side)')
    );
    await fireEvent.click(getByText('Save note'));

    // The refusal lands where the reviewer is looking, and the composer stays
    // open with the text they typed rather than discarding it.
    expect(getByRole('alert').textContent).toContain('diff revision abc123');
    expect(getByText('Save note')).toBeDefined();
    // captureDigest travels with the note: it is what lets the runtime refuse
    // rather than pin the note to whatever is on that line now.
    expect(dispatch).toHaveBeenCalledWith(
      expect.objectContaining({ type: 'addReviewNote', captureDigest: 'abc123', line: 2 })
    );
    dispatch.mockRestore();
  });

  it('VerifySurface reports explicit evidence counts and store-failure gaps', () => {
    clientStore.snapshot = {
      ...baseSnapshot,
      storeFailure: 'durable store unavailable',
      messages: [
        {
          kind: 'tool',
          id: 'tool-1',
          name: 'cargo test',
          outcome: 'ok',
          detail: 'exit status 0',
          detailTruncated: false
        },
        {
          kind: 'tool',
          id: 'tool-2',
          name: 'cargo clippy',
          outcome: 'failed',
          detail: 'lint failure',
          detailTruncated: false
        }
      ]
    };

    const { getByText, getAllByText, queryByText } = render(VerifySurface);

    expect(getAllByText('1')).toHaveLength(2);
    expect(getByText('Succeeded')).toBeDefined();
    expect(queryByText('Verified')).toBeNull();
    expect(getByText('durable store unavailable')).toBeDefined();
    expect(
      getByText(/The desktop contract still lacks explicit verification-command exit statuses/)
    ).toBeDefined();
  });

  it('AttentionSurface dispatches approval decisions from explicit snapshot state', async () => {
    const dispatchSpy = vi.spyOn(clientStore, 'dispatch');

    clientStore.snapshot = {
      ...baseSnapshot,
      pendingApproval: {
        id: 'approval-1',
        toolName: 'run_command',
        tier: 'execute',
        preview: 'cargo test'
      }
    };

    const { getByText } = render(AttentionSurface);

    await fireEvent.click(getByText('Approve once'));

    expect(dispatchSpy).toHaveBeenCalledWith({
      type: 'resolveApproval',
      approval: 'approval-1',
      decision: 'approve-once'
    });
  });

  it('AttentionSurface prioritizes durability failure before recovery and approval', () => {
    clientStore.snapshot = {
      ...baseSnapshot,
      storeFailure: 'append failed',
      recovery: {
        state: 'required',
        run: 'run-1',
        kind: 'uncertain-effect',
        summary: 'A previous effect needs recovery.',
        effectIsCertain: false
      },
      pendingApproval: {
        id: 'approval-1',
        toolName: 'run_command',
        tier: 'execute',
        preview: 'cargo test'
      }
    };

    const { getAllByText } = render(AttentionSurface);
    const priorities = getAllByText(/^Priority \d$/).map((node) => node.textContent);

    expect(priorities.slice(0, 3)).toEqual(['Priority 1', 'Priority 2', 'Priority 3']);
  });
});
