# Project context and Agent Skills contract

Phase 5 adds advisory repository knowledge without widening smed's deterministic authority boundary. Project prose and activated skills can influence a provider request; they cannot approve tools, weaken path containment, or make a bundled script executable.

Sources:

- <https://agentskills.io/specification>
- <https://agentskills.io/client-implementation/adding-skills-support>
- Agent Skills reference repository commit `38a2ff82958afee88dadf4831509e6f7e9d8ef4e` (2026-07-09), used only to derive independent conformance cases.

Verified: 2026-07-16.

## Project instructions

smed canonicalises the project root and working directory before discovery. The working directory must remain inside the project root. It then walks from root toward the working directory and, at each level, loads:

1. `AGENTS.md` as canonical advisory context.
2. `CLAUDE.md` as additional advisory context.

More specific directories appear later in the provider prompt. `SMED.md` is not discovered; smed introduces no competing prose standard. Instruction reads are bounded to 512 KiB in total, and a canonical path that leaves the project root is rejected with a typed diagnostic.

Instruction text and activated skill bodies are XML-entity escaped before insertion into smed's model-facing frames. Advisory content may still influence the model, but text such as `</instruction_file>` or `</skill_content>` cannot terminate smed's declared framing early.

This loader is intentionally an advisory-context input, not a policy parser. A future structured-gates input can sit beside it without treating prose as enforceable or making memory a second source of truth.

## Skill discovery and precedence

Only immediate child directories of these roots are candidates, in this order:

1. Project `.smed/skills`.
2. Project `.agents/skills`.
3. User native smed config `skills` directory.
4. User `~/.agents/skills`.

Entries within a root are sorted by filename. The first valid skill name wins; every later collision is ignored with `SCHEMA_INVALID`. Locations are absolute canonical paths. Missing roots are normal. Unreadable, invalid, escaping, and truncated inputs become typed diagnostics rather than disappearing silently.

Discovery retains only `name`, `description`, canonical location, and project/user scope. It does not put the body or resource contents in model context. Default bounds are 256 skill directories, 256 KiB per `SKILL.md`, 256 resource paths per activation, resource traversal depth three, and 512 KiB of project instructions. `.git`, `.venv`, `node_modules`, `target`, `vendor`, and `__pycache__` are never traversed as resources.

## Frontmatter validation

`SKILL.md` must begin with a closed YAML frontmatter document. smed accepts the published fields `name`, `description`, `license`, `compatibility`, `allowed-tools`, and `metadata`, rejects unknown fields and wrong YAML types, and enforces the published character limits. Names are NFKC-normalised before lowercase, character-set, length, and directory-name checks.

`allowed-tools` is parsed only to establish that the document is structurally valid. It is never translated into a permission or policy decision.

## Progressive activation and trust

The provider initially receives project instructions plus the skill catalog. It can request one catalog entry through `activate_skill`; only then does smed load that skill's full body. Local resource paths are listed, but their contents remain on demand and remote resources are never fetched automatically.

The first project-scope activation is forced through a human approval regardless of the current read policy. A successful, durably recorded activation marks the workspace trusted for later project-scope activations in that session, including after restarting and resuming that same session. This is consent to reload advisory context, not side-effect authority: writes, commands, and bundled scripts still cross their ordinary policy gates. User-scope skills need no workspace trust prompt.

Activation succeeds only after re-canonicalising and re-validating the skill. A skill moved outside its root or changed incompatibly since discovery is refused with a stable `ReasonCode`. The durable `ToolCompleted` result is the source of truth for both the activated instructions and `SkillActivated` effect. Checkpoints cache the activated-name set and workspace trust bit; tail replay derives the same state from that effect.

Resource listing also fails closed when a path is dangling, unreadable, or escapes the allowed root. smed does not silently skip such a path and present an incomplete manifest as complete. Count and depth limits are different: those produce an explicit `<truncated>true</truncated>` marker.

Bundled scripts are inert resource paths. Running one is a separate `run_command` proposal and crosses the ordinary execute-tier policy, approval, path, environment, and evidence gates. Skill metadata never grants that authority.

## Plugin manifests

Third-party plugin manifests are discovered per-file as `.smed/plugins/*.yaml`
(workspace root and user config directory, bounded scan in
`src/context/plugins.rs`). Each file is a fail-closed ADR-0016 manifest;
discovery makes a plugin visible and inspectable, registration and execution
require explicit owner authorisation (`src/plugins/`, `ToolTier::Execute`-pinned).
See `smed plugin create --help`, `smed plugin list`, and the flagship example
`examples/plugins/vercel-deployments/`.

## Operator surface

Type `/skills` in an idle composer to toggle the catalog overlay. It distinguishes available and active entries, project and user scope, workspace-trust state, and typed discovery diagnostics. It is a projection of the runtime snapshot, not another registry.
