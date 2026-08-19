# Rename sweep — the delegable remainder

**Status:** Open. Prerequisite work is landed; this is the cosmetic tail.
**Related:** [ADR-0018](adr/0018-rename-smed-to-mjolnr.md), `tests/branding.rs`

---

## Why this document exists

ADR-0018 says the mechanical bulk of the rename should be done by a low-cost
model, with a human on the trust-critical seams. That split only works if
"trust-critical" is something a machine can check. It now is: `tests/branding.rs`
fails the build on a stale **contract** — a message naming a command that does
not exist, a child branch prefixed `smed/`, a `SMED_` environment variable
presented as canonical, a release script packaging a binary that is not built.

Those are done. What is left is **prose**: the old name sitting in sentences. It
is not wrong in the way a bad command is wrong, and it does not fail anything.
It is also the majority of the remaining occurrences, which is exactly why it
should not consume expensive review.

## What is already done

| Surface | State |
|---|---|
| Package, binary, Tauri bundle | `mjolnr` |
| Workspace config dir | `.mjolnr/`, `.smed/` read-fallback + migrate-on-write |
| User config dir (theme, onboarding marker) | single resolver, `smed` fallback |
| Subagent / external-agent branches | `mjolnr/sub-*`, `mjolnr/ext-*` |
| Environment prefix | `MJOLNR_<ID>_BASE_URL`, legacy `SMED_` still read |
| CLI instructions in messages and help | `mjolnr <subcommand>` |
| `.gitignore`, `scripts/install.sh`, release workflow | `mjolnr` |
| Guard | `tests/branding.rs`, 5 tests, green |

## What is left

Approximate counts at the time of writing:

| Area | Files | Occurrences | Character |
|---|---|---|---|
| `docs/` | ~36 | ~410 | Design docs, phase plans, ADR bodies |
| `desktop/src/` | ~20 | ~120 | User-visible copy and component tests |
| `src/` prose | many | ~180 | Doc comments and module headers |
| `mockups/`, `examples/` | ~5 | ~55 | Fixture copy, plugin example README |

### Rules for the sweep

1. **Do not touch internal code names.** `smed::` crate paths, module names, and
   types stay (ADR-0018 §3). Renaming them is a separate, later decision.
2. **Do not touch the allowlist in `tests/branding.rs`.** Each entry names a
   place where the old name is the correct answer; the reasons are in the file.
3. **Historical references stay historical.** ADR-0018, this file, and
   `renaming-to-mjolnr.md` describe a rename — they must keep saying `smed`.
4. **Rewrite, do not substitute, where the sentence depends on the old name.**
   These need a human or a careful rewrite, not `sed`:
   - `desktop/src/lib/components/chrome/AppEmblem.svelte` — the mark is an anvil
     because *smed* is Danish for "smith". Under the new name the anvil is
     orphaned. Needs a decision, not a find-and-replace.
   - `docs/POSITIONING.md` and `README.md` — any place the name is argued for.
   - `desktop/src/routes/+page.svelte` — the header still renders `smed-says`,
     which is two names stale (it predates `smed` as well).
5. **Green gates, one commit:** `cargo fmt --all -- --check`,
   `cargo clippy --all-targets --all-features -- -D warnings`,
   `cargo test --all-features`, `cargo deny check`, and the desktop suite.

### Prompt to hand over

> Sweep the remaining cosmetic `smed` → `mjolnr` references in `docs/`,
> `desktop/src/`, `mockups/`, `examples/`, and Rust doc comments. Follow
> `docs/rename-sweep.md` rules 1–5 exactly. Do not change internal `smed::`
> module or type names. Do not edit `tests/branding.rs` or its allowlist. Leave
> the four rewrite-not-substitute items in rule 4 alone and list them in your
> report instead. Gates green, one commit.

## Done when

`renaming-to-mjolnr.md` can honestly say **Migrated**, and the emblem decision in
rule 4 is either made or explicitly deferred with a note saying so.
