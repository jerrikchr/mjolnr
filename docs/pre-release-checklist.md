# Pre-release checklist

*Gates that remain before this repository is made public.*

- **Repository identity** — secure the GitHub org/name `mjolnr` and a home domain
  (`mjolnr.dev` or `mjolnr.sh`). ADR-0018 records the rename and leaves domain
  availability explicitly non-blocking.
- **First-impression surfaces** — a repository description and topics, and a
  screenshot or demo capture showing a governed refusal.
- **Emblem** — the current mark is an anvil inherited from the `smed` (Danish
  for "smith") era. It is a placeholder twice over now: chosen for legibility,
  and no longer matching the name. ADR-0018's mythic vocabulary is the brief.
- **One clean history** — the public repository begins with one reviewed root
  commit and contains no unreachable internal commit references, private
  worktree paths, transcripts, credentials, or captured provider responses.
- **Documentation truth pass** — README, positioning, current capability tables
  (including task sources — GitHub/Linear/Vercel/Supabase — and plugin scaffold
  inventory: `mjolnr plugin create <name> [--template node|rust|python] [--yes]`,
  `mjolnr plugin list`, `.mjolnr/plugins/*.yaml`), release instructions, and security
  limitations agree on what is implemented, what is development-only, and what
  remains roadmap. `cargo deny check` passes on all four targets (`deny.toml`), and
  `THIRD_PARTY.md` last-verified date is ≥ last integration/plugin merge.
- **Terminal release proof** — run the full Rust verification matrix, build each
  advertised artifact target, verify checksums, and smoke-test installation from
  the produced binary rather than `cargo run`.
- **Desktop boundary** — either package and verify the Tauri client or state
  consistently that it is included as development source only. Review Tauri
  capabilities, content-security policy, frontend checks, Rust bridge tests, and
  a packaged macOS journey before claiming a desktop release.
- **Security language** — verify every public surface distinguishes policy gates,
  path containment, external provenance, and any future OS containment. No page
  may imply sandboxing or verified external-agent work that the runtime cannot
  prove.
- **Provenance and dependencies** — `THIRD_PARTY.md`, lockfiles, licences, source
  policy, and generated assets are complete and reproducible from public inputs.
- **Release workflow** — exercise the hosted workflow from the clean root,
  inspect its permissions, and confirm it does not publish a crate or create a
  public release without an explicit owner action.
- **crates.io / npm / PyPI** — names are free, but publishing the crate is a
  separate decision that has not been made; keep `publish = false`.

- **Rename sweep closed out** — `tests/branding.rs` is green (it guards the
  contract surfaces: commands, branch prefixes, environment prefix, release
  scripts), and the cosmetic prose backlog in `docs/rename-sweep.md` is either
  done or explicitly accepted as shipping with stale wording.
