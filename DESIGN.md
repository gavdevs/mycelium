# Mycelium — Design

A graph-aware code intelligence layer for AI coding agents.

## What it is

Mycelium is a local-first code intelligence service that watches a set of repositories, parses their source into a property graph of symbols and relationships, embeds each symbol with a synthesized natural-language description, and exposes the result through a CLI that AI coding agents (initially Claude Code) invoke via a skill.

The core thesis: AI coding agents currently explore codebases by reading files and grepping. This is expensive in tokens and unreliable in retrieval. A pre-computed graph + embeddings layer turns "find the relevant code" from minutes of file-reading into a single sub-second query that returns the precise symbols an agent needs, with their structural neighborhood already attached. The published research on this approach (Codebase-Memory, arXiv 2603.27277) measures the win at roughly 10x fewer tokens and 2x fewer tool calls per task.

Mycelium is what that approach looks like with three additions the published work doesn't have: graph-augmented semantic retrieval, accumulated personalization that compounds with use, and a skill-based interface that doesn't pay the MCP token tax.

## Non-goals

Mycelium is explicitly not:

- A coding agent itself. Claude Code, Codex, Cursor, etc. are the agents. Mycelium gives them better retrieval.
- A Language Server Protocol implementation. Mycelium *uses* language servers internally during indexing — coordinated via multilspy — to refine call/reference edges with type-aware precision. It does not implement, expose, or proxy an LSP at the agent surface. Tools like Serena cover LSP-precision queries at the agent surface; Mycelium covers semantic and graph-augmented retrieval. The two compose.
- A general-purpose graph database. FalkorDB is the storage layer; Mycelium is the indexing pipeline and query CLI on top.
- A cloud service. Everything runs locally by default — embedding, description synthesis, and reranking. The model layer is provider-agnostic: each role (`Embedder`, `Synthesizer`, `Reranker`) is a swappable trait, and Ollama is the shipping default. Cloud providers (Anthropic, OpenAI, Qwen-cloud, etc.) and remote self-hosted Ollama endpoints are configurable per role, but never the default. The token-savings thesis only holds when the internal pipeline is local; cloud routing is an escape hatch, not a paved road.
- An IDE plugin. The interface is a CLI invoked by an agent's bash tool, not an editor extension.

## System overview

Three components, deliberately kept separable:

**The indexer daemon.** A long-running Rust process that watches registered repositories via filesystem events, runs the indexing pipeline on changes, and maintains the FalkorDB graph for each repo. Owns the heavyweight work: parsing, embedding generation, description synthesis. Talks to local Ollama instances for model inference.

**The CLI.** A Rust binary, `mycel`, that opens a FalkorDB connection, runs queries, and prints results. No daemon dependency at query time — the binary reads the graph directly. This is what agents invoke.

**The skill.** A markdown document at `.claude/skills/mycel/SKILL.md` per repo (or globally at `~/.claude/skills/mycel/`). Teaches Claude Code when to reach for `mycel` and how to interpret its output. ~500 tokens; massive savings versus an MCP server's tool registration overhead.

The split matters because indexing is heavy and benefits from a long-running process (cached models, warm filesystem watcher), while querying is light and should be invoked per-request from the CLI without involving the daemon.

**Model-provider abstraction.** Every model call goes through a small trait surface — `Embedder`, `Synthesizer`, `Reranker` — implemented for `Ollama` in v0 and extensible to cloud providers later. The trait split matters because the three roles have different swap costs: switching embedder requires a full reindex (vector dimension is sticky), switching synthesizer affects only future descriptions, and switching reranker is free at query time. Configuration picks one provider *per role*, not one provider for everything.

## Hardware tiers and model selection

Mycelium supports a wide range of dev hardware, from 8GB unified-memory laptops to 64GB+ workstations. The defaults are organized into three tiers; the bootstrap script detects available memory and picks one. Users override via `MYCEL_TIER` or `[models] tier =` in config.

| Tier | Target hardware | Embedder | Reranker | Synthesizer |
|------|-----------------|----------|----------|-------------|
| `minimal` | 8GB MBA-class | embeddinggemma (300M, ~200MB) | qwen3-reranker:0.6b (~600MB) | gemma4:e2b (Q4, ~1.5GB) |
| `balanced` | 16GB | embeddinggemma | qwen3-reranker:0.6b | gemma4:e4b (Q4, ~3GB) |
| `max` | 32GB+ | embeddinggemma | qwen3-reranker:4b | qwen3.6:35b-a3b (Q4, ~17GB) |

The embedder is **the same model across all tiers** — EmbeddingGemma at 768d. This is deliberate: vector dimension is tied to the FalkorDB vector index at build time, and a tier change should never force a reindex. EmbeddingGemma fits comfortably even on `minimal`, MRL-truncatable down to 128d if storage matters more than retrieval quality.

The reranker upgrades on `max` because the swap is free (applied at query time, no reindex). The synthesizer's tier choice is the most consequential — quality of description synthesis scales meaningfully with model size, and `minimal` users get a smaller model with the option to point `synthesizer.endpoint` at a remote Ollama or cloud provider when description quality matters.

Reranker pick is an empirical question Phase 4 will settle: build a small eval harness against real Mycelium-shape queries and compare candidates (qwen3-reranker, gte-reranker-modernbert-base via sidecar, mxbai-rerank-v2 via sidecar, jina-reranker-v2 via sidecar). The default may change once the eval is in place. The trait abstraction makes swaps trivial.

## Storage architecture

FalkorDB as the graph + vector store. One Redis instance with the FalkorDB module loaded, one graph per indexed repo, namespaced as `mycel:<repo-name>`. Cypher for queries, GraphBLAS-backed traversal under the hood.

Why FalkorDB specifically: it's the most actively maintained embedded-style graph DB targeting AI/GraphRAG workloads as of early 2026. Kuzu, the other obvious candidate, was archived in October 2025 after Apple acquired the team. FalkorDB's traversal is sub-millisecond on the queries Mycelium issues, vector search lives in the same graph as the structural data (no sync problem), and the Redis-module deployment model is operationally trivial — one local Docker container, started and torn down via `just up`/`just down`.

The license is AGPL-3.0 community / commercial enterprise. For local CLI use this is functionally equivalent to MIT. Anyone who wants to host Mycelium-as-a-service would need to release modifications, which is appropriate for an OSS tool.

### Schema

**Node types:**

- `File` — every source file. Properties: path, language, last_modified, content_hash.
- `Symbol` — every named entity. Subtypes via `kind` property: `function`, `method`, `class`, `interface`, `type`, `component`, `hook`, `constant`, `module`. Properties: name, qualified_name, file_path, start_line, end_line, signature, jsdoc, synthesized_description, exported (bool), embedding (vector property), body_hash (blake3 of the signature+body slice used for embedding), description_source_hash (the body_hash captured when synthesized_description was written; NULL when no description; used to detect staleness on edit).
- `Commit` — git commits. Properties: sha, author, timestamp, message_normalized.

**Edge types:**

- `DEFINES` — file → symbol. Trivial structural edge.
- `CALLS` — symbol → symbol. Function calls, method invocations.
- `IMPORTS` — file → symbol (external) or file → file (internal). Both forms tracked.
- `EXTENDS` / `IMPLEMENTS` — class/type inheritance and interface implementation.
- `USES_TYPE` — symbol → type. Captures type references in signatures and bodies.
- `REFERENCES` — symbol → symbol. Generic reference, weaker than CALLS. Used for type aliases, re-exports, prop drilling.
- `RE_EXPORTS` — file → symbol. Barrel-file pattern; tracked separately because barrel files lie about who really owns a symbol.
- `CO_CHANGED` — symbol → symbol, with `count` and `last_seen` properties. Derived from commit history; not from static analysis. Captures coupling that imports don't reveal.
- `TESTED_BY` — symbol → symbol. Computed at index time by walking forward from test entrypoints through CALLS edges. Answers "what test exercises this code path."
- `MODIFIED_IN` — symbol → commit. Maintains git-aware queries like "what's changed in this area recently."

The schema is deliberately denormalized for retrieval speed. We're optimizing for graph traversal queries, not transactional writes.

### Vector index

Each `Symbol` node carries an `embedding` property — a 768-dimension float32 vector produced by EmbeddingGemma (Google's 300M-parameter on-device embedding model, Apache 2.0, Matryoshka-trained, native 768d with optional truncation to 128d). FalkorDB's vector index handles cosine similarity search natively; `db.idx.vector.queryNodes()` returns ranked symbols against a query vector. The active embedder identity and dimension are stored as graph metadata and used to gate `mycel reindex --embedder=<new>` migrations — accidental embedder swaps are refused rather than silently producing a corrupted index.

The choice to embed *synthesized descriptions* rather than raw signatures is the central retrieval-quality decision. Raw signatures and bodies cluster by surface syntax — embeddings of two unrelated React components both look "similar" because they both define components. Descriptions cluster by behavior — "validates and parses an OAuth bearer token" and "checks request authentication" embed close because they're semantically close, even if the symbols look nothing alike. This matters most on codebases with sparse comments (most production codebases), which is exactly when the agent needs the retrieval most.

## Indexing pipeline

Six stages, run sequentially per file changed (in parallel across files):

**1. Parse (tree-sitter).** Tree-sitter walks the AST. Extracts symbols (with file/line ranges, signatures, JSDoc), call sites, imports, type references, class hierarchy. **TypeScript/TSX/JavaScript/JSX and Rust in v0** — TypeScript covers the canonical web codebase target; Rust is the dogfooding target, since Mycelium is itself a Rust project and indexing its own source as it grows is the most realistic test. Codebase-Memory's 66-language coverage is the long-term reference target. Per-language extraction strategies live in `crates/mycel-extract/src/languages/<language>.rs` and follow a common trait. This is the floor — fast, multi-language, no external server dependencies.

**2. Refine (LSP via multilspy).** The daemon runs a long-running multilspy subprocess (Python wrapping language-server-protocol clients) and queries it for type-aware edge resolution on each parsed file. multilspy returns precise definition/reference/call info from the actual language server (`tsserver` for TS/TSX/JS/JSX, `rust-analyzer` for Rust), upgrading edges that tree-sitter could only resolve heuristically. Edges carry a `source` property (`tree-sitter` or `lsp`) so the graph remembers which resolver produced each edge. Languages without a configured LSP fall back to tree-sitter alone — heuristic but functional.

**3. Build graph.** Refined edges upsert into FalkorDB. The original 6-strategy heuristic pipeline (qualified-name match, imported-symbol resolution, re-export following, method receiver resolution, generic-name match, unresolved-placeholder) survives as the tree-sitter-only fallback path for languages without LSP coverage. With multilspy + a real type checker available, most edges are `source: lsp`.

**4. Embed.** Each new or changed symbol gets embedded. EmbeddingGemma running on local Ollama produces 768-dim vectors over `signature + body[:60 lines]` (capped at ~1500 tokens; pure-signature was insufficient — too many symbols share useless names like `process` or `handle`). Batched in groups of 32 for throughput. Vectors written into the graph as node properties alongside `body_hash`. The active embedder identity and dimension are recorded once as graph metadata and gate future embedder swaps.

**5. (Removed from the indexing hot path as of the 2026-05-05 redirection.)** Description synthesis no longer runs as a stage of `mycel index`. Instead, behavioral descriptions are written workload-driven by Claude Code itself (via the shipped `mycel-graph-care` skill calling `mycel set-description`) or, optionally, by a manual `mycel synthesize` bulk pass. See `docs/superpowers/specs/2026-05-05-workload-driven-synthesis-design.md` for the full rationale; the tier-selected Synthesizer infrastructure (gemma4:e2b/e4b, qwen3.6:35b-a3b) is preserved unchanged for the bulk-pass path.

**6. Update derived edges.** `CO_CHANGED` and `TESTED_BY` edges are recomputed for affected subgraphs. Co-change analysis walks the recent commit history (last 200 commits as a default window) and increments edge counts; old commits decay via a recency factor. Test reachability does a forward BFS from each test file's exported symbols, recording every symbol reached as `TESTED_BY` with the test as source.

### Incremental update strategy

Cold first index of a 250K-LOC codebase like demand-ui (on `max` tier hardware), as of the 2026-05-05 redirection: ~5–15 minutes, dominated by embedding throughput rather than synthesis (synthesis is no longer a stage of cold indexing — it's workload-driven; see Phase 2 below). On `minimal` tier, expect 2–3× that figure. Subsequent indexes are content-hash-keyed and skip unchanged files entirely regardless of tier.

The daemon watches via `notify` (Rust filesystem watcher) for fast feedback during active development, with a debounce of 2 seconds to coalesce burst-write events from build tools. Git push events are also subscribable through a `mycel hook install` command that adds a post-commit hook calling `mycel reindex --since HEAD~1`.

A full re-index runs nightly to catch anything the watcher missed (unmounted volumes, missed events, etc.) and to refresh `CO_CHANGED` edges with the latest commit data.

## Query layer

The `mycel` CLI exposes the four tiers of queries we identified during design:

**Tier 1 — direct relationships:**

- `mycel callers <symbol>` — who calls this
- `mycel callees <symbol>` — what this calls
- `mycel definers <symbol>` — where defined (handles ambiguous names)
- `mycel uses <type>` — where a type is used
- `mycel implements <interface>` — concrete implementations
- `mycel imports <file>` — who imports from this file

**Tier 2 — multi-hop reachability:**

- `mycel blast-radius <symbol> [--depth N]` — transitive callers, useful for "what does changing this affect"
- `mycel context <symbol> [--depth N]` — symbol + its 1-2 hop neighborhood, dense form, ready for prompt injection
- `mycel path <from> <to>` — shortest path between two symbols
- `mycel surface <seed>` — expands outward from a seed until natural boundaries (route handlers, exported APIs); returns "the feature"

**Tier 3 — structural patterns:**

- `mycel similar <symbol>` — structurally and semantically similar code elsewhere
- `mycel canonical <pattern>` — most common implementation of a pattern in this codebase
- `mycel recent <symbol> [--days N]` — what's changed near this symbol recently
- `mycel tests <symbol>` — what tests exercise this code path
- `mycel dead` — unreachable code from known entrypoints

**Tier 4 — graph-augmented semantic retrieval:**

- `mycel find <natural-language query>` — the headline command. Pipeline:
  1. Embed the query
  2. Vector search returns top 50 candidate symbols
  3. Graph expansion: pull each candidate's 1-2 hop neighborhood
  4. Rerank the expanded set (~200 symbols) with the configured reranker (default `qwen3-reranker:0.6b` on minimal/balanced tiers, `qwen3-reranker:4b` on max), conditioned on the original query
  5. Return top 8-12 with descriptions, file paths, and line ranges

This is the query Claude Code will use most. It's also the query the design is structured around — Tiers 1-3 fall out of the same graph and embeddings.

Every command supports `--json` for structured output and a default human-readable mode for terminal use. JSON output schemas are stable and versioned; the skill doc references them.

## Personalization layer

The piece that turns Mycelium from "a code intelligence tool" into "a code intelligence tool that knows you." Five accumulating knowledge layers:

**Repo knowledge.** The graph itself. Built passively, refined incrementally. Already covered above.

**Convention knowledge.** Per-repo patterns extracted from commit history and code samples. Bootstrap pass on first index runs the configured synthesizer (tier-selected) over the most-recent 50 PRs and a sample of source files, outputting `~/.local/share/mycel/<repo>/conventions.md` — patterns like "this codebase uses named exports, never default exports," "tests live in `__tests__/` directories," "components always have a paired `.stories.tsx` file." These get injected into the agent's context via the skill when invoked.

**Personal knowledge.** Cross-repo style preferences. Bootstrap pass samples the user's last 30 commits across all indexed repos and synthesizes a `~/.config/mycel/style.md`. Refined when the user runs `mycel correct <symbol> "<note>"` to record a manual correction. The corrections accumulate into a delta log that periodically gets summarized into rules.

**Task knowledge.** Episodic memory. When an agent invokes `mycel find` or `mycel context`, the query and the symbols returned are logged. When the agent later commits changes, the diff is matched against the retrieval log to record which retrievals "succeeded" (the modified files appeared in retrieval results). This produces a per-repo retrieval-quality trace that informs reranking weights over time.

**Failure knowledge.** When verify gates fail (typecheck, lint, tests) on agent-produced code, the failure pattern + the eventual fix get recorded. `mycel similar-failure <error>` retrieves prior fixes for similar failures. This is the layer that turns "this looks like the bug from last Tuesday" into a precise retrieval.

These layers compose in queries: a `mycel find` invocation surfaces relevant code symbols *plus* applicable conventions *plus* applicable style rules *plus* relevant prior tasks. The agent gets a much richer prompt than it could construct on its own, in a single tool call.

## Claude Code integration

The skill at `.claude/skills/mycel/SKILL.md` is the integration surface. Structure:

1. **When to use Mycelium.** Triggers: "find the code that does X," "where is Y defined," "what calls Z," any task starting on an unfamiliar part of the codebase. Anti-triggers: tasks already scoped to a single known file, simple syntactic edits.

2. **Command reference.** Brief one-line summary of each command, with the exact CLI invocation. Claude executes via the Bash tool.

3. **Output interpretation.** How to read JSON output, what each field means, when to follow up with `mycel context` or `mycel callers` for deeper exploration.

4. **Composition guidance.** "Start with `mycel find` for natural-language exploration, then `mycel context` to expand the most relevant symbols, then read full files only for symbols you'll actually modify."

The skill doc deliberately doesn't try to be exhaustive — it teaches the pattern, not every command's edge cases. Claude is good enough to figure out edge cases from `mycel <command> --help`.

### Token measurement

To prove the system actually saves tokens, the skill includes a directive: at the end of each task, the agent summarizes which `mycel` commands it ran and roughly how many tokens those queries returned. This produces a passive measurement trail. A `mycel stats` command rolls these up to per-task and per-repo averages, so we can compare against baseline (file-reading-and-grep) numbers.

The hypothesis: 5-10x token reduction on tasks that involve substantial codebase exploration, no measurable difference on tasks that are pure-edit. If the data doesn't show this within a month of real use, the system is theatre and we know.

## Performance targets

For a codebase the size of demand-ui (~250K LOC, ~2.5K files, estimated 75-150K symbols):

- Cold first index: under 60 minutes including description synthesis
- Incremental update on a single-file change: under 5 seconds
- `mycel find` end-to-end (embed query → vector search → graph expand → rerank): under 500ms
- `mycel callers` / `mycel callees` / Tier 1 queries: under 50ms
- Database size on disk: under 1GB

These are working targets, not contractual SLAs. Real numbers come from running the thing on real code. Targets above assume `max` tier hardware; cold first index on `minimal` (8GB MBA) is realistically 2-3× slower owing to smaller synthesizer models, smaller embed batch sizes, and more frequent model load/unload cycles. Query-side latencies (`mycel find`, Tier 1 queries) are largely tier-independent — the embedder and reranker are small and fast across tiers, and graph traversal is FalkorDB doing the work.

## Sequenced build plan

Five phases, each producing a usable artifact.

**Phase 1 — symbol graph, Tier 1 queries, and `mycel find`.** The full v0 surface: Cargo workspace skeleton (9 crates), FalkorDB local-container infra, tree-sitter + multilspy extraction for TS/TSX/JS/JSX, FalkorDB schema with vector index, EmbeddingGemma integration via the `Embedder` trait (Ollama provider), `mycel index`, all Tier 1 commands (`callers`, `callees`, `definers`, `imports`, `uses`, `implements`), and `mycel find` returning vector-search top-K. As of the 2026-05-05 Phase 2 revision, embeddings are computed against `signature + body slice` (first ~60 lines, capped at ~1500 tokens) rather than pure signature — bad names like `process` or `handle` are too common for signature-only embeddings to distinguish symbols reliably. The compressed Phase 1+2 of the original plan — ships the token-savings thesis as the *first* shippable artifact rather than as a follow-on. ~3-4 weekends, or whatever an agent run takes.

**Phase 2 — workload-driven description synthesis.** Synthesizer trait wired to tiered Ollama defaults (gemma4:e2b/e4b, qwen3.6:35b-a3b), description generation pipeline with 1-hop graph context, re-embed on descriptions. *But* synthesis runs lazily, not eagerly: cold indexing does no synthesis at all (Cursor-style: just embed signature+body), and behavioral descriptions are written by Claude Code itself as a side-effect of normal work — when Claude has read a function's body and reasoned about it for the user's task, it writes a 1–3 sentence description back to the graph via `mycel set-description`, instructed by a shipped skill (`mycel-graph-care`). `mycel synthesize` remains as a manual bulk fallback for non-Claude workflows. `mycel find` quality emerges over time, scoped to the regions of the codebase users actually work in.

> *Why "workload-driven" instead of eager.* The originally-shipped Phase 2 (2026-05-04) ran the Synthesizer over every Symbol at index time. Dogfood result: 30 minutes to index this workspace, projecting to multiple days for a typical 50k-symbol work codebase. The behavioral-search thesis was right; the trigger was wrong. Synthesizing every symbol pre-pays for retrievals nobody will ever issue. The 2026-05-05 redirection moves synthesis to "where Claude is already working, paid for by the session that's already running" — see `docs/superpowers/specs/2026-05-05-workload-driven-synthesis-design.md` for the full rationale and design.

**Phase 3 — graph-augmented retrieval, reranking, and the cross-file edge unblock.** Five workstreams (see `docs/superpowers/specs/2026-05-10-phase-3-reranking-and-tier-4-design.md`):

1. *Cross-file edge resolution* — rewrite the multilspy bridge's reference path so call sites resolve to Symbol qnames (not file URIs); CALLS / USES_TYPE / IMPLEMENTS edges finally land cross-file. Prerequisite for graph expansion.
2. *Reranker integration* — `OllamaReranker::rerank` implemented against qwen3-reranker (yes/no-token scoring via Ollama `/api/generate` with logits), per-tier defaults (`0.6b` minimal/balanced, `4b` max), graceful cosine-fallback on failure.
3. *Full Tier 4 pipeline* — new `find_tier4`: vector top-50 → 1-hop graph expand on CALLS/USES_TYPE/IMPLEMENTS → cap ~200 → rerank → top-10. Phase 1's vector-only `find` stays as `--no-rerank`.
4. *Query-time on-demand synthesis* — when cosine scores are low and flat, synthesize behavioral descriptions for borderline candidates inline, re-embed, re-rank, persist back to the graph. Workload-driven Phase 2 applied to retrieval.
5. *Reranker eval harness* — ~30 hand-curated behavioral queries against this repo with ground-truth qnames; nDCG@10 / MRR / hit-rate@5 per reranker; promotes winner to default if it beats the current default without regressing latency past 500ms p50.

**Phase 4 — git-derived edges and Tier 3 queries.** Co-change analysis, test reachability, `mycel similar` / `mycel canonical` / `mycel recent`. Personalization layer's task knowledge bootstrap also lives here — query/result logging starts here, retrieval-quality traces accumulate.

**Phase 5 — personalization and skill.** Convention extraction, style memory, failure knowledge, the skill doc itself. Token measurement framework. Cloud provider implementations (Anthropic, OpenAI, Qwen-cloud) drop into the `mycel-models` crate as additional `Embedder`/`Synthesizer`/`Reranker` impls — no architecture change needed. Ongoing iteration on the skill.

Phase 1 is usable in isolation and *is the product thesis*. Each subsequent phase is additive — nothing rewrites earlier work. The order is chosen so the most leverage lands earliest: a working symbol graph + signature-embedded `find` (Phase 1) already saves tokens; description synthesis (Phase 2) lifts quality; the rest compounds from there.

## Open questions

Things deliberately deferred:

- **Cross-language graph edges.** What does it mean for a TypeScript file to call a C# endpoint? Out of scope for v1.
- **Multi-language description synthesis.** Phase 2's synthesizer is assumed to handle whatever language we throw at it. Reasonable for TS/JS; needs validation for other languages when we add them.
- **Reranker pick.** Default is `qwen3-reranker:0.6b/4b` (Ollama-native, well-benchmarked). Phase 3's eval harness (`crates/mycel-query/eval/`) sweeps Ollama-native candidates by default and sidecar candidates (`gte-reranker-modernbert-base`, `mxbai-rerank-v2`, `jina-reranker-v2`) under `--with-sidecar-rerankers`. Default may move based on results, weighted against the operational cost of running a sidecar.
- **Reranker fine-tuning.** Whether a small fine-tune on accumulated retrieval-quality traces would meaningfully improve quality is an open empirical question, separate from picking the off-the-shelf default.
- **MCP frontend.** A wrapper that exposes Mycelium's CLI as MCP tools is a reasonable addition for users on tools other than Claude Code (Cursor, Codex, Aider). Skill+CLI is the v1 default; MCP is a thin shim we can add later without changing the backend.
- **Multi-tenant deployments.** Mycelium is single-user for v1. Sharing an index across a team would require auth, access control, and a different storage tenancy model — all real work, all out of scope.
- **Repo-local, distributable indexes.** A future pivot where `<repo>/.mycel/` is the source of truth (JSONL symbols/edges + binary embeddings + manifest), with FalkorDB as a hydrate-on-start cache. Would enable "clone repo with mycel installed → graph just works" and `mycel pack`/`unpack` for sharing prebuilt indexes. Architecture doesn't preclude it (embedder identity already in graph metadata, schema versioned, pipeline reproducible) but defer until Phase 5+ when real collaboration patterns surface — at which point the right format will be much clearer from usage.
- **Editor integration.** No VSCode extension, no IntelliJ plugin in v1. The CLI is the universal interface.

## Status

This document is the specification as of project start. The repository is `gavdevs/mycelium`. License: TBD pending FalkorDB AGPL-3.0 implications; default plan is AGPL-3.0 to match. Author: Gav (`gavdevs`). Conceptual debts to: Codebase-Memory (Vogel et al., arXiv 2603.27277) for the tree-sitter knowledge-graph approach and call-resolution insights, Serena (oraios) for proving the symbol-aware retrieval pattern at scale, and the FalkorDB team for an actively-maintained graph DB at exactly the right shape for this use case.
