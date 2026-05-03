# Mycelium — Design

A graph-aware code intelligence layer for AI coding agents.

## What it is

Mycelium is a local-first code intelligence service that watches a set of repositories, parses their source into a property graph of symbols and relationships, embeds each symbol with a synthesized natural-language description, and exposes the result through a CLI that AI coding agents (initially Claude Code) invoke via a skill.

The core thesis: AI coding agents currently explore codebases by reading files and grepping. This is expensive in tokens and unreliable in retrieval. A pre-computed graph + embeddings layer turns "find the relevant code" from minutes of file-reading into a single sub-second query that returns the precise symbols an agent needs, with their structural neighborhood already attached. The published research on this approach (Codebase-Memory, arXiv 2603.27277) measures the win at roughly 10x fewer tokens and 2x fewer tool calls per task.

Mycelium is what that approach looks like with three additions the published work doesn't have: graph-augmented semantic retrieval, accumulated personalization that compounds with use, and a skill-based interface that doesn't pay the MCP token tax.

## Non-goals

Mycelium is explicitly not:

- A coding agent itself. Claude Code, Codex, Cursor, etc. are the agents. Mycelium gives them better retrieval.
- A Language Server Protocol implementation. Tools like Serena cover LSP-precision queries; Mycelium covers semantic and graph-augmented retrieval. The two compose.
- A general-purpose graph database. FalkorDB is the storage layer; Mycelium is the indexing pipeline and query CLI on top.
- A cloud service. Everything runs locally. Embedding models are local. Description synthesis is local. Cloud model support may be added later as an opt-in routing option, but never as a default.
- An IDE plugin. The interface is a CLI invoked by an agent's bash tool, not an editor extension.

## System overview

Three components, deliberately kept separable:

**The indexer daemon.** A long-running Rust process that watches registered repositories via filesystem events, runs the indexing pipeline on changes, and maintains the FalkorDB graph for each repo. Owns the heavyweight work: parsing, embedding generation, description synthesis. Talks to local Ollama instances for model inference.

**The CLI.** A Rust binary, `mycel`, that opens a FalkorDB connection, runs queries, and prints results. No daemon dependency at query time — the binary reads the graph directly. This is what agents invoke.

**The skill.** A markdown document at `.claude/skills/mycel/SKILL.md` per repo (or globally at `~/.claude/skills/mycel/`). Teaches Claude Code when to reach for `mycel` and how to interpret its output. ~500 tokens; massive savings versus an MCP server's tool registration overhead.

The split matters because indexing is heavy and benefits from a long-running process (cached models, warm filesystem watcher), while querying is light and should be invoked per-request from the CLI without involving the daemon.

## Storage architecture

FalkorDB as the graph + vector store. One Redis instance with the FalkorDB module loaded, one graph per indexed repo, namespaced as `mycel:<repo-name>`. Cypher for queries, GraphBLAS-backed traversal under the hood.

Why FalkorDB specifically: it's the most actively maintained embedded-style graph DB targeting AI/GraphRAG workloads as of early 2026. Kuzu, the other obvious candidate, was archived in October 2025 after Apple acquired the team. FalkorDB's traversal is sub-millisecond on the queries Mycelium issues, vector search lives in the same graph as the structural data (no sync problem), and the Redis-module deployment model is operationally trivial — one container on Isengard.

The license is AGPL-3.0 community / commercial enterprise. For local CLI use this is functionally equivalent to MIT. Anyone who wants to host Mycelium-as-a-service would need to release modifications, which is appropriate for an OSS tool.

### Schema

**Node types:**

- `File` — every source file. Properties: path, language, last_modified, content_hash.
- `Symbol` — every named entity. Subtypes via `kind` property: `function`, `method`, `class`, `interface`, `type`, `component`, `hook`, `constant`, `module`. Properties: name, qualified_name, file_path, start_line, end_line, signature, jsdoc, synthesized_description, exported (bool), embedding (vector property).
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

Each `Symbol` node carries an `embedding` property — a 768-dimension float32 vector produced by Qwen3-Embedding-0.6B with Matryoshka truncation from its native 1024d. FalkorDB's vector index handles cosine similarity search natively; `db.idx.vector.queryNodes()` returns ranked symbols against a query vector.

The choice to embed *synthesized descriptions* rather than raw signatures is the central retrieval-quality decision. Raw signatures and bodies cluster by surface syntax — embeddings of two unrelated React components both look "similar" because they both define components. Descriptions cluster by behavior — "validates and parses an OAuth bearer token" and "checks request authentication" embed close because they're semantically close, even if the symbols look nothing alike. This matters most on codebases with sparse comments (most production codebases), which is exactly when the agent needs the retrieval most.

## Indexing pipeline

Five stages, run sequentially per file changed (in parallel across files):

**1. Parse.** Tree-sitter walks the AST. Extracts symbols (with file/line ranges, signatures, JSDoc), call sites, imports, type references, class hierarchy. TypeScript/TSX/JavaScript/JSX in v0; Codebase-Memory's 66-language coverage is the long-term reference target. Per-language extraction strategies live in `src/extractors/<language>.rs` and follow a common trait.

**2. Build graph.** Newly extracted symbols upsert into FalkorDB. Edges resolve via a 6-strategy call resolution pipeline borrowed conceptually from Codebase-Memory (the engineering insight here is real and worth crediting):

1. Exact qualified-name match within file scope
2. Imported-symbol resolution via the import graph
3. Module re-export following
4. Method receiver resolution (TypeScript `this`, class methods)
5. Generic-name match within reachable scope (last-resort)
6. Unresolved — recorded as a placeholder edge for later passes

This is the boring-but-load-bearing part of the system. Get it wrong and the call graph is full of noise; get it right and Tier 4 retrieval becomes precise.

**3. Embed.** Each new or changed symbol gets embedded. Qwen3-Embedding-0.6B running on local Ollama. Batched in groups of 32 for throughput. Vectors written into the graph as node properties.

**4. Synthesize descriptions.** Qwen3.6-35B-A3B running on local Ollama generates a one-paragraph description per symbol. Input: signature + JSDoc + the bodies of immediate callers and callees (1-hop graph context — using the graph we just built to inform descriptions about each symbol's role). The 262K context window means we can stuff substantial context per generation. Output: 2-3 sentences capturing what the symbol does, what it returns, and what its role is in the codebase. These descriptions are then re-embedded — the embedding step runs *twice* in v1, once on raw signatures for fast first-pass retrieval and once on synthesized descriptions for high-quality retrieval. v0 ships with signature-only embeddings to get a working system; descriptions land in phase 3.

**5. Update derived edges.** `CO_CHANGED` and `TESTED_BY` edges are recomputed for affected subgraphs. Co-change analysis walks the recent commit history (last 200 commits as a default window) and increments edge counts; old commits decay via a recency factor. Test reachability does a forward BFS from each test file's exported symbols, recording every symbol reached as `TESTED_BY` with the test as source.

### Incremental update strategy

Cold first index of a 250K-LOC codebase like demand-ui: 30-60 minutes, dominated by description synthesis. Subsequent indexes are content-hash-keyed and skip unchanged files entirely.

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
  4. Rerank the expanded set (~200 symbols) with Qwen3-Reranker-4B, conditioned on the original query
  5. Return top 8-12 with descriptions, file paths, and line ranges

This is the query Claude Code will use most. It's also the query the design is structured around — Tiers 1-3 fall out of the same graph and embeddings.

Every command supports `--json` for structured output and a default human-readable mode for terminal use. JSON output schemas are stable and versioned; the skill doc references them.

## Personalization layer

The piece that turns Mycelium from "a code intelligence tool" into "a code intelligence tool that knows you." Five accumulating knowledge layers:

**Repo knowledge.** The graph itself. Built passively, refined incrementally. Already covered above.

**Convention knowledge.** Per-repo patterns extracted from commit history and code samples. Bootstrap pass on first index runs Qwen3.6 over the most-recent 50 PRs and a sample of source files, outputting `~/.local/share/mycel/<repo>/conventions.md` — patterns like "this codebase uses named exports, never default exports," "tests live in `__tests__/` directories," "components always have a paired `.stories.tsx` file." These get injected into the agent's context via the skill when invoked.

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

These are working targets, not contractual SLAs. Real numbers come from running the thing on real code.

## Sequenced build plan

Six phases, each producing a usable artifact.

**Phase 1 — symbol graph and Tier 1 queries.** Tree-sitter parser for TS/TSX/JS/JSX, FalkorDB schema, `mycel index` command, `mycel callers/callees/imports`. Usable for navigation tasks. ~2 weekends.

**Phase 2 — embeddings and `mycel find`.** Qwen3-Embedding integration, vector index, hybrid retrieval (vector + lexical via FalkorDB full-text search). `mycel find` works on signature-embedded symbols. Token savings start showing up here. ~1 weekend.

**Phase 3 — synthesized descriptions.** Qwen3.6-35B-A3B integration, description generation pipeline, re-embedding on descriptions. Retrieval quality jumps. Slow first index, fast queries forever after. ~1 weekend.

**Phase 4 — graph-augmented retrieval and reranking.** Qwen3-Reranker integration, the full Tier 4 pipeline. `mycel find` becomes the headline command. ~1 weekend.

**Phase 5 — git-derived edges and Tier 3 queries.** Co-change analysis, test reachability, `mycel similar` / `mycel canonical` / `mycel recent`. ~1 weekend.

**Phase 6 — personalization and skill.** Convention extraction, style memory, the skill doc itself. Token measurement framework. ~1 weekend, plus ongoing iteration on the skill.

Phase 1 is usable in isolation. Each subsequent phase is additive — nothing rewrites earlier work. The order is chosen so the most leverage lands earliest: a working symbol graph (Phase 1) is already more capable than `grep`, and `mycel find` (Phase 2) is already saving tokens.

## Open questions

Things deliberately deferred:

- **Cross-language graph edges.** What does it mean for a TypeScript file to call a C# endpoint? Out of scope for v1.
- **Multi-language description synthesis.** Phase 3 assumes the synthesizer model handles whatever language we throw at it. Reasonable for TS/JS; needs validation for other languages when we add them.
- **Reranker fine-tuning.** Off-the-shelf Qwen3-Reranker is the v1 plan. Whether a small fine-tune on accumulated retrieval-quality traces would meaningfully improve quality is an open empirical question.
- **MCP frontend.** A wrapper that exposes Mycelium's CLI as MCP tools is a reasonable addition for users on tools other than Claude Code (Cursor, Codex, Aider). Skill+CLI is the v1 default; MCP is a thin shim we can add later without changing the backend.
- **Multi-tenant deployments.** Mycelium is single-user for v1. Sharing an index across a team would require auth, access control, and a different storage tenancy model — all real work, all out of scope.
- **Editor integration.** No VSCode extension, no IntelliJ plugin in v1. The CLI is the universal interface.

## Status

This document is the specification as of project start. The repository is `gavdevs/mycelium`. License: TBD pending FalkorDB AGPL-3.0 implications; default plan is AGPL-3.0 to match. Author: Gav (`gavdevs`). Conceptual debts to: Codebase-Memory (Vogel et al., arXiv 2603.27277) for the tree-sitter knowledge-graph approach and call-resolution insights, Serena (oraios) for proving the symbol-aware retrieval pattern at scale, and the FalkorDB team for an actively-maintained graph DB at exactly the right shape for this use case.
