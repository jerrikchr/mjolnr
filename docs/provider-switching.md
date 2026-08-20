# Same-session provider and model switching

mjolnr owns canonical history, tool state, project state, skills, budgets, and persistence. Providers own only the wire exchange that produced the next canonical assistant turn. That boundary is what makes switching possible without pretending hidden state migrates.

## Operator command

Switch only while idle:

```text
/model anthropic claude-sonnet-5
/model openai gpt-4.1
```

The requested provider/model must be registered and declare both streaming and tool support. A switch during a provider stream, approval wait, or tool execution is refused durably with `RUN_ACTIVE`; an unknown or incapable route is refused with `PROVIDER_INCOMPATIBLE_MODEL`. Neither refusal changes the active model or sends a provider request.

Successful selection persists `ModelChanged` before changing live state. Resume projects the same provider/model from that event.

## What crosses the boundary

- User-visible user and assistant text.
- Tool calls and their provider correlation IDs.
- Tool results, stable reason codes, truncation, and evidence IDs.
- Activated skill instructions already present in canonical tool-result history.
- Project root, policy, read set, mutation/evidence state, usage, and budgets owned by the runtime/checkpoint.

Each adapter translates that canonical history directly into its own wire objects. Anthropic gets assistant `tool_use` and user `tool_result` blocks; OpenAI gets response function-call items and outputs. Neither adapter translates through the other's types.

## What does not cross

Provider-private reasoning/thinking blocks, signatures, response cache handles, and hidden server state are not canonical. The TUI emits this notice on a successful switch:

```text
MODEL CHANGED // anthropic:claude-sonnet-5 // provider-private reasoning and cache state were not migrated
```

Dropping private state is deliberate. Reconstructing thinking blocks, editing signatures, or claiming cache continuity would be both lossy and false.

## Verified synthetic transcript

The headless integration path in `tests/provider_switching.rs` exercises this sequence:

```text
alpha:alpha-1
  user      inspect the repository
  assistant activate_skill(switch-context), read_file(note.txt)
  tool      activated full skill instructions
  tool      observed durable repository state
  assistant alpha continued

ModelChanged -> beta:beta-1

beta:beta-1
  receives the complete canonical alpha history above
  user      continue from the prior work
  assistant beta continued
```

Assertions prove the project root, activated-skill set, skill body, read result, correlation IDs, prior provider attribution, and transcript remain available. A separate capability test proves an incompatible switch writes a typed refusal and sends zero requests.
