<script lang="ts">
  import { clientStore } from '$lib/runtime/client.svelte';
  import * as Alert from '$lib/components/ui/alert';
  import { Badge } from '$lib/components/ui/badge';
  import { Button } from '$lib/components/ui/button';
  import * as Card from '$lib/components/ui/card';
  import * as Empty from '$lib/components/ui/empty';
  import { HugeiconsIcon } from '@hugeicons/svelte';
  import { FileDiffIcon } from '@hugeicons/core-free-icons';

  let snap = $derived(clientStore.snapshot);
  let changes = $derived(snap.changes);
  let threads = $derived(snap.reviewThreads.items);

  // Which line the composer is open on, as `path␟side␟line`. Purely
  // presentation state: the note itself only exists once the runtime accepts it.
  let composerKey = $state<string | null>(null);
  let draft = $state('');
  let noteRefusal = $state<string | null>(null);
  let selected = $state<Record<string, boolean>>({});
  let sendRefusal = $state<string | null>(null);
  // A reply is attached to an existing durable thread only after the runtime
  // accepts it. Keeping one composer open at a time keeps the surface bounded
  // without putting draft text into the authoritative snapshot.
  let replyThreadId = $state<string | null>(null);
  let replyDraft = $state('');
  let replyRefusal = $state<string | null>(null);

  const lineKey = (path: string, side: 'old' | 'new', line: number) =>
    `${path}␟${side}␟${line}`;

  function threadsAt(path: string, side: 'old' | 'new', line: number) {
    return threads.filter(
      (thread) =>
        thread.anchor.path === path && thread.anchor.side === side && thread.anchor.line === line
    );
  }

  function openComposer(path: string, side: 'old' | 'new', line: number) {
    composerKey = lineKey(path, side, line);
    draft = '';
    noteRefusal = null;
  }

  async function saveNote(path: string, side: 'old' | 'new', line: number) {
    if (!changes) return;
    // captureDigest is the diff revision on screen. Sending it back is what
    // lets the runtime refuse a note whose diff has moved instead of pinning it
    // to whatever occupies that line now.
    const refusal = await clientStore.dispatch({
      type: 'addReviewNote',
      path,
      side,
      line,
      captureDigest: changes.captureDigest,
      body: draft
    });
    // `dispatch` answers with `null` on success and a typed refusal otherwise.
    // The refusal is shown at the line it belongs to rather than only in the
    // global alert: "that diff has moved" is about this note, and a reviewer
    // needs it where they are looking.
    if (refusal) {
      noteRefusal = refusal.message;
      return;
    }
    composerKey = null;
    draft = '';
    noteRefusal = null;
  }

  let selectedIds = $derived(threads.filter((t) => selected[t.id]).map((t) => t.id));

  async function sendSelected() {
    const refusal = await clientStore.dispatch({
      type: 'sendReviewNotes',
      threadIds: selectedIds
    });
    sendRefusal = refusal ? refusal.message : null;
    if (!refusal) selected = {};
  }

  function openReply(threadId: string) {
    replyThreadId = threadId;
    replyDraft = '';
    replyRefusal = null;
  }

  async function saveReply(threadId: string) {
    const refusal = await clientStore.dispatch({
      type: 'addReviewComment',
      threadId,
      body: replyDraft
    });
    if (refusal) {
      replyRefusal = refusal.message;
      return;
    }
    replyThreadId = null;
    replyDraft = '';
    replyRefusal = null;
  }
</script>

<div class="flex h-full flex-col gap-4 overflow-y-auto p-4" data-testid="changes-surface">
  <header class="flex flex-col justify-between gap-3 sm:flex-row sm:items-start">
    <div class="flex flex-col gap-1">
      <h1 class="text-2xl font-semibold tracking-tight">Changes</h1>
      <p class="text-muted-foreground text-sm">
        Exact file changes and line-level review.
      </p>
    </div>
    {#if changes}
      <div class="flex flex-col items-start gap-1 sm:items-end">
        <Badge variant="outline">
          {changes.files.length} file{changes.files.length === 1 ? '' : 's'} changed
        </Badge>
        <!--
          The freshness marker, not a currency claim. smed re-reads git on
          explicit triggers only, so the honest statement is when it last
          looked — never that this is what the tree contains now.
        -->
        <span class="text-muted-foreground text-xs">
          captured at #{changes.captureSequence}
        </span>
      </div>
    {/if}
  </header>

  {#if snap.pendingApproval}
    <Alert.Root>
      <Alert.Title>Pending exact effect</Alert.Title>
      <Alert.Description>
        <div class="flex flex-col gap-3">
          <p>smed is waiting on human approval before the next side effect is allowed.</p>
          <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
            <dt class="text-muted-foreground">Tool</dt>
            <dd><code>{snap.pendingApproval.toolName}</code></dd>
            <dt class="text-muted-foreground">Tier</dt>
            <dd>{snap.pendingApproval.tier}</dd>
          </dl>
          <pre class="bg-muted overflow-x-auto rounded-md p-3 text-xs whitespace-pre-wrap">{snap.pendingApproval.preview}</pre>
        </div>
      </Alert.Description>
    </Alert.Root>
  {/if}

  {#if !changes || changes.files.length === 0}
    <Empty.Root class="border border-dashed">
      <Empty.Header>
        <Empty.Media variant="icon">
          <HugeiconsIcon icon={FileDiffIcon} strokeWidth={2} />
        </Empty.Media>
        <Empty.Title>No governed change records yet</Empty.Title>
        <Empty.Description>
          No explicit file changes are present in the active snapshot.
        </Empty.Description>
      </Empty.Header>
    </Empty.Root>
  {:else}
    <div class="flex flex-col gap-4">
      <div class="flex items-center gap-2">
        <span class="text-sm font-medium">State:</span>
        <Badge variant={changes.state === 'proposed' ? 'default' : 'secondary'}>{changes.state}</Badge>
      </div>

      {#if changes.filesTruncated}
        <Alert.Root>
          <Alert.Title>This change set is partial</Alert.Title>
          <Alert.Description>
            Files were dropped at smed's projection bound, or git's own output was cut. What is
            below is part of the working tree, not all of it — review it as such.
          </Alert.Description>
        </Alert.Root>
      {/if}

      {#if changes.undiffedUntracked.length > 0}
        <Alert.Root>
          <Alert.Title>
            {changes.undiffedUntracked.length} untracked path{changes.undiffedUntracked.length === 1
              ? ''
              : 's'} not diffed
          </Alert.Title>
          <Alert.Description>
            <p class="mb-2">
              Untracked directories, and files past the per-refresh bound, are named rather than
              read. They are listed so the surface can say what it is not showing.
            </p>
            <ul class="list-inside list-disc font-mono text-xs">
              {#each changes.undiffedUntracked as path (path)}
                <li>{path}</li>
              {/each}
            </ul>
          </Alert.Description>
        </Alert.Root>
      {/if}

      {#if threads.length > 0}
        <div class="flex flex-col gap-2 rounded-md border p-3">
          <div class="flex flex-wrap items-center gap-3">
            <span class="text-sm font-medium">
              {threads.length} review note{threads.length === 1 ? '' : 's'}
            </span>
            <Button size="sm" disabled={selectedIds.length === 0} onclick={sendSelected}>
              Send {selectedIds.length} to smed
            </Button>
          </div>
          <!--
            What sending is, said plainly. It is a message, not an instruction
            smed must obey and not an approval: the ordinary gates still apply
            to anything it does about them.
          -->
          <p class="text-muted-foreground text-xs">
            Sending posts the selected notes as your message in this session. It approves nothing
            and changes no policy, and smed is never marked as having addressed a note.
          </p>
          {#if snap.reviewThreads.truncated}
            <p class="text-muted-foreground text-xs">
              More notes exist than smed carries in one snapshot; not all are shown.
            </p>
          {/if}
          {#if sendRefusal}
            <p class="text-destructive text-xs" role="alert">{sendRefusal}</p>
          {/if}
        </div>
      {/if}

      {#if changes.readEvidence.length > 0}
        <div class="flex flex-col gap-1 text-sm text-muted-foreground">
          <span class="font-medium">Read before edit evidence:</span>
          <ul class="list-inside list-disc">
            {#each changes.readEvidence as ev}
              <li>
                <span class="font-mono">{ev.path}</span> at rev {ev.readRevision.slice(0, 7)}
                (tool event: {ev.toolEventId})
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      {#each changes.files as file (file.path)}
        <Card.Root>
          <Card.Header class="flex-row items-start justify-between bg-muted/50 py-3">
            <div class="flex items-center gap-2">
              <span class="font-mono text-sm font-semibold">{file.path}</span>
              {#if file.status !== 'modified'}
                <Badge variant="outline">{file.status}</Badge>
              {/if}
              {#if file.oldPath}
                <span class="text-xs text-muted-foreground">← {file.oldPath}</span>
              {/if}
            </div>
            <div class="flex gap-2">
              {#if file.content === 'binary'}<Badge variant="outline">Binary</Badge>{/if}
              {#if file.content === 'undecodable'}<Badge variant="outline">Not UTF-8</Badge>{/if}
              {#if file.isLarge}<Badge variant="outline">Large</Badge>{/if}
              {#if file.isTruncated}<Badge variant="outline">Truncated</Badge>{/if}
            </div>
          </Card.Header>
          <Card.Content class="p-0">
            {#if file.hunks.length > 0}
              <div class="flex flex-col font-mono text-xs">
                {#each file.hunks as hunk}
                  <div class="bg-muted px-4 py-1 text-muted-foreground border-y border-border/50">
                    {hunk.header}
                  </div>
                  <div class="flex flex-col overflow-x-auto">
                    {#each hunk.lines as line}
                      {@const isAdded = line.kind === 'added'}
                      {@const isRemoved = line.kind === 'removed'}
                      <!--
                        A note pins to one side. A removed line only exists on
                        the old side and an added line only on the new, so the
                        side is decided by which number the line actually has —
                        never guessed from the line number alone.
                      -->
                      {@const side = line.newLineNumber != null ? 'new' : 'old'}
                      {@const anchoredLine = line.newLineNumber ?? line.oldLineNumber}
                      <div class="flex hover:bg-muted/50 {isAdded ? 'diff-added' : isRemoved ? 'diff-removed' : ''}">
                        <div class="w-10 shrink-0 select-none border-r border-border/50 px-2 text-right text-muted-foreground opacity-50">
                          {line.oldLineNumber ?? ''}
                        </div>
                        <div class="w-10 shrink-0 select-none border-r border-border/50 px-2 text-right text-muted-foreground opacity-50">
                          {line.newLineNumber ?? ''}
                        </div>
                        <div class="w-6 shrink-0 select-none px-2 text-center">
                          {isAdded ? '+' : isRemoved ? '-' : ' '}
                        </div>
                        <div class="flex-1 whitespace-pre px-2">{line.content}</div>
                        {#if anchoredLine != null}
                          <button
                            type="button"
                            class="text-muted-foreground hover:text-foreground shrink-0 px-2 text-xs"
                            aria-label="Add a review note on {file.path} line {anchoredLine} ({side} side)"
                            onclick={() => openComposer(file.path, side, anchoredLine)}
                          >
                            +note
                          </button>
                        {/if}
                      </div>

                      {#if anchoredLine != null}
                        {#each threadsAt(file.path, side, anchoredLine) as thread (thread.id)}
                          <div
                            class="border-border/50 bg-muted/30 flex flex-col gap-1 border-y px-4 py-2"
                            data-testid="review-thread"
                          >
                            <div class="flex flex-wrap items-center gap-2">
                              <input
                                type="checkbox"
                                id="select-{thread.id}"
                                checked={selected[thread.id] ?? false}
                                onchange={(event) => {
                                  selected = {
                                    ...selected,
                                    [thread.id]: event.currentTarget.checked
                                  };
                                }}
                              />
                              <label for="select-{thread.id}" class="text-xs font-medium">
                                Note on {thread.anchor.side} line {thread.anchor.line}
                              </label>
                              <Badge variant="outline">{thread.status}</Badge>
                              {#if thread.anchorStale}
                                <!--
                                  The note stays on the line it was written
                                  against. It is marked, not moved and not
                                  hidden: relocating it silently is the failure
                                  the anchor's recorded revision exists to make
                                  impossible.
                                -->
                                <Badge variant="outline">
                                  Diff has moved since this note was written
                                </Badge>
                              {/if}
                            </div>
                            {#each thread.comments as comment}
                              <p class="text-sm whitespace-pre-wrap">{comment.body}</p>
                              {#if comment.bodyTruncated}
                                <span class="text-muted-foreground text-xs">
                                  This note was longer than smed carries and is shown in part.
                                </span>
                              {/if}
                            {/each}
                            {#if thread.commentCountTruncated}
                              <span class="text-muted-foreground text-xs">
                                Not every comment on this thread is shown.
                              </span>
                            {/if}
                            {#if thread.responseMessageId}
                              <span class="text-muted-foreground text-xs">
                                smed answered in message {thread.responseMessageId}. That is a
                                reply, not a claim that the note was addressed.
                              </span>
                            {/if}
                            <Button
                              size="sm"
                              variant="ghost"
                              class="w-fit"
                              aria-label="Reply to review thread"
                              onclick={() => openReply(thread.id)}
                            >
                              Reply
                            </Button>
                            {#if replyThreadId === thread.id}
                              <div class="border-border/50 flex flex-col gap-2 border-t pt-2">
                                <label class="text-xs font-medium" for="reply-body-{thread.id}">
                                  Add a reply to this thread
                                </label>
                                <textarea
                                  id="reply-body-{thread.id}"
                                  class="border-input bg-background min-h-16 rounded-md border p-2 font-sans text-sm"
                                  aria-label="Reply body"
                                  bind:value={replyDraft}
                                ></textarea>
                                {#if replyRefusal}
                                  <p class="text-destructive text-xs" role="alert">{replyRefusal}</p>
                                {/if}
                                <div class="flex gap-2">
                                  <Button size="sm" onclick={() => saveReply(thread.id)}>
                                    Add reply
                                  </Button>
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    onclick={() => {
                                      replyThreadId = null;
                                      replyDraft = '';
                                      replyRefusal = null;
                                    }}
                                  >
                                    Cancel
                                  </Button>
                                </div>
                              </div>
                            {/if}
                          </div>
                        {/each}

                        {#if composerKey === lineKey(file.path, side, anchoredLine)}
                          <div class="border-border/50 flex flex-col gap-2 border-y px-4 py-3">
                            <label class="text-xs font-medium" for="note-body">
                              Note on {side} line {anchoredLine} of {file.path}
                            </label>
                            <textarea
                              id="note-body"
                              class="border-input bg-background min-h-16 rounded-md border p-2 font-sans text-sm"
                              bind:value={draft}
                            ></textarea>
                            {#if noteRefusal}
                              <p class="text-destructive text-xs" role="alert">{noteRefusal}</p>
                            {/if}
                            <div class="flex gap-2">
                              <Button
                                size="sm"
                                onclick={() => saveNote(file.path, side, anchoredLine)}
                              >
                                Save note
                              </Button>
                              <Button
                                size="sm"
                                variant="ghost"
                                onclick={() => {
                                  composerKey = null;
                                  noteRefusal = null;
                                }}
                              >
                                Cancel
                              </Button>
                            </div>
                          </div>
                        {/if}
                      {/if}
                    {/each}
                  </div>
                {/each}
              </div>
            {:else}
              <!--
                Why there is no diff matters more than the fact of it: each
                cause has a different remedy, so each says its own sentence
                instead of sharing one vague line.
              -->
              <div class="p-4 text-center text-sm text-muted-foreground">
                {#if file.content === 'binary'}
                  git reports this file as binary and does not produce a text diff.
                {:else if file.content === 'undecodable'}
                  git returned bytes that are not valid UTF-8. smed will not guess at an encoding,
                  so no content is shown.
                {:else if file.status === 'added'}
                  Added, with no content to diff.
                {:else}
                  No printable diff hunks.
                {/if}
              </div>
            {/if}
          </Card.Content>
        </Card.Root>
      {/each}
    </div>
  {/if}
</div>
