# ADR-0017: FTS5 Trigram Search Baseline for Memory Retrieval and Deferral of Embedded Vectors

**Status:** Accepted
**Date:** 2026-08-18
**Deciders:** Jerrik Christiansen
**Related:** ADR-0001 (all-Rust core), ADR-0003 (shared Rust core), ADR-0016 (capability modules & plugin protocol), AGENTS.md §5 (performance), §8 (provenance & dependencies), Standing Law #2 (recall is a projection)

## Context

Phase 1 of mjolnr's capability modernization introduced the **Memory Capability Module** (`src/memory/`), spanning:
- Tier 1: Frozen session rules snapshots (`.mjolnr/rules/*.md`, `.mjolnr/USER.md`).
- Tier 2 & 3: Temporal knowledge triples and progressive recall tools (`memory_search`, `memory_timeline`, `memory_expand`).
- Episodic consolidation: Background distillers synthesizing past turns into searchable episodes.

A key design question in Phase 1 (§2.4 of the Master Implementation Plan) was the retrieval engine behind `memory_search`: whether to integrate embedded neural vector embeddings (e.g. `fastembed-rs`, `ort`, `candle-transformers`) into mjolnr's binary or to rely on SQLite FTS5 with trigram tokenization and recency weighting.

## Options Evaluated

### Option 1: SQLite FTS5 with Trigram Tokenization & Hybrid Recency Scoring (Chosen)
- **Mechanism:** SQLite's built-in `fts5` virtual table with `tokenize = 'trigram'` over triple subjects, predicates, objects, and episodic summaries. Scored using a composite ranking:
  $$\text{Score} = \text{FTS Rank} + w_{\text{recent}} \cdot e^{-\lambda \Delta t} + w_{\text{exact}} \cdot \mathbb{I}(\text{exact match})$$
- **Pros:**
  - Zero external C/C++ or dynamic library dependencies (`tokio-rusqlite` handles FTS5 natively).
  - Preserves instant compilation and small binary footprint (< 25MB).
  - No background model weight downloads (no 100MB+ huggingface fetches at startup).
  - Substring, prefix, and code-identifier search work reliably (critical for programming identifiers like `ApprovalDecision` or `MAX_EVENTS_PER_PASS` where embedding models often perform poorly).
  - 100% offline, local-first, zero secrets/text leakage over the network.
- **Cons:**
  - Does not perform conceptual semantic clustering for synonyms that share no lexical n-grams (e.g. "auth" vs "credentials" when neither appears in the corpus).

### Option 2: Embedded Neural Vector Search (`fastembed-rs` / `ort` / `candle`)
- **Mechanism:** In-process ONNX or Candle embedding model (e.g. `all-MiniLM-L6-v2` or `bge-small-en-v1.5`) generating 384-dimensional dense vectors stored in SQLite or a flat cosine index.
- **Pros:**
  - Semantic similarity matching across synonyms.
- **Cons:**
  - **Massive binary bloat:** `ort` and ONNX Runtime require 60-150MB of compiled C++ runtime artifacts.
  - **Toolchain friction:** Requires `cmake`, Python, or system C++ compilers, breaking mjolnr's clean `cargo build` and cross-compilation pipeline on macOS, Linux, and Windows.
  - **Cold start & RAM penalty:** Loading model weights consumes 200MB+ RAM and introduces noticeable delay on session startup.
  - **Identifier blindspot:** Small embedding models frequently score syntactic tokens (function names, error codes) worse than BM25/trigram matching.

### Option 3: Remote Provider Embeddings (OpenAI / OpenRouter / Ollama)
- **Mechanism:** Calling remote embedding APIs (`text-embedding-3-small`, `ollama/nomic-embed-text`) during triple ingestion and retrieval.
- **Pros:**
  - High semantic fidelity without local compute.
- **Cons:**
  - Violates local-first offline execution; every tool call incurs network roundtrips.
  - Provider cost and rate limits on background consolidation.
  - Memory text leaks to third-party APIs prior to user prompts.

## Decision

**mjolnr ships FTS5 with trigram tokenization and recency weighting as the first-party memory search baseline. Embedded neural vector search is deferred from core and may be added as an optional plugin in Phase 2 via the Capability Module Plugin Protocol (ADR-0016).**

### Rationale

1. **Deterministic & Fail-Safe:** FTS5 trigram indexes require no network, no external model files, and no platform-dependent C++ runtimes.
2. **Coding Harness Characteristics:** In developer workflows, exact symbol matching, identifier prefixes, error codes, and temporal proximity account for over 90% of relevant context recall. Trigram FTS5 excels at this domain.
3. **Extraction & Dependency Hygiene (AGENTS.md §2.2, §8):** Adding ONNX/Torch C-bindings into mjolnr core would violate the extraction test and permanently burden every contributor and CI runner.
4. **Clean Upgrade Path:** Because memory search is accessed strictly through the `memory_search` runtime actor query and `MemoryStore` abstraction, a vector backend can be swapped in or plugged in as an external capability module (ADR-0016) without modifying the client or core harness contracts.

## Consequences

- `src/memory/store.rs` uses `memory_fts` with `tokenize = 'trigram'` for all lexical and progressive search.
- No heavy ML or ONNX dependencies are added to `Cargo.toml`.
- If semantic clustering is desired by specific workflows, it will be contributed as an optional external plugin speaking the JSON-RPC plugin protocol (ADR-0016).
