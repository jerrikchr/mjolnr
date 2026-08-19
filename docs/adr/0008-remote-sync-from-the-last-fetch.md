# ADR-0008: Compute remote sync locally, labelled as of the last fetch

**Status:** Accepted
**Date:** 2026-07-30
**Deciders:** Jerrik Christiansen
**Phase:** D5

## Context

`RepositoryState.remote_sync` has carried `RepositorySyncState::Unknown`
unconditionally since the D5 producer landed. The stated reason:

> Comparing against a remote requires a fetch, which is a network side effect no
> D5 trigger performs; a `Synced` derived from a stale local ref would be a claim
> about a remote nobody contacted.

The first half is right and the second half overreaches, which is how a
permanently empty field ended up looking like a principled position.

`git rev-list --left-right --count HEAD...@{upstream}` performs **no network
I/O**. It reads `refs/remotes/<remote>/<branch>` from the object database — a ref
that a previous `fetch`, `pull`, or `clone` wrote. The counts it returns are
exact statements about two commits that are both present locally. What they do
*not* describe is the remote as it stands now.

So there are three distinct states, and the producer was collapsing all of them
into `Unknown`:

| | Knowable offline? |
|---|---|
| Working tree differs from `HEAD` | Yes — already reported as `dirty_count` |
| Local branch differs from its last-fetched upstream | **Yes** — this ADR |
| Remote has moved since that fetch | No, and no read can make it so |

The mockup that prompted this (ADR-0007) rendered the middle row as a bare
`synced` in the verified colour, which is the third row's claim wearing the
second row's evidence.

## Decision

Compute ahead/behind from the local remote-tracking ref, and label the result
with the moment the ref was written rather than with the moment it was read.

- `Ahead`, `Behind`, and `Diverged` are populated from
  `git rev-list --left-right --count HEAD...@{upstream}`, argv only, through the
  existing `repository::git` chokepoint. No fetch, no `pull`, no network.
- `Unknown` is retained and now means what it says: no upstream is configured,
  or git would not answer. It stops being the value for "we did not look".
- **`Synced` is never rendered as a bare word.** Zero ahead and zero behind means
  "identical to the ref last fetched", and the surface says that, qualified. The
  DTO variant keeps its name for wire compatibility; the *rendering* carries the
  qualifier.
- The qualifier is not decorative and not optional. A surface showing sync state
  also shows that the position is as of the last time smed saw the remote.

  **The qualifier does not depend on a timestamp existing.** Implementation
  measured what git actually provides: `git reflog show @{upstream}` answers
  after a fetch or a push (the entry reads `update by push`), and answers
  *nothing* in a fresh clone, which writes the tracking ref without a reflog
  entry. So `remote_sync_as_of` is `Option`, `None` is an ordinary case, and the
  honest sentence — "as of the last time smed saw the remote" — is carried by
  the *variant's meaning* rather than by the timestamp. When a timestamp is
  available it sharpens the statement; it never licenses dropping it.

  This is narrower than this ADR's first draft, which said a surface "shows when
  the upstream ref was last updated". That promised a value git does not always
  have.
- Governance colour: sync state is **not** `--gov-verified`. Being level with a
  ref fetched an hour ago is not a verified state, and
  `tauri-design-system.md` forbids a component claiming one.

## Alternatives rejected

**Keep `Unknown` forever.** Rejected: it discards information smed holds, and a
field that is structurally incapable of a value is worse than an honest one — it
trains a reader to ignore the row, and eventually someone deletes it.

**Fetch on a refresh trigger to make it current.** Rejected, firmly. A read path
that reaches the network turns opening a project into an authenticated remote
call, on a credential the user did not offer for that purpose, at a latency the
UI cannot bound. If smed ever fetches it will be an explicit, human-initiated,
governed operation with its own approval — not a side effect of rendering a
panel.

**Report the ref's own age instead of ahead/behind.** Rejected: "last fetched two
days ago" answers a different question than "you have three commits they do not".
Both are wanted, which is why the decision keeps the counts *and* the marker.

## Accepted costs

- One additional `git` invocation per refresh, on a path that already runs two.
  It is a ref read, not an object walk of any size that matters here.
- The wire variant is named `Synced` while the rendering must never say "synced"
  unqualified. That is a real trap for the next person writing a surface, and it
  is called out in the variant's own doc comment rather than left to this ADR.
- A repository whose upstream ref is months stale will report small counts
  confidently. The counts are true; the marker is what stops them from being
  misleading, so a surface that drops the marker to save a line has broken the
  contract, not tidied it.
