# Mycelium — Phase 3: Reranking, Full Tier 4, and the Cross-File Edge Unblock

**Status**: Approved design as of 2026-05-10. Builds on Phase 1 (shipped) and Phase 2's 2026-05-05 workload-driven redirection (shipped). Phase 3 is the first phase that *requires* the Phase 2 redirection to make sense — graph-augmented retrieval is only interesting if the graph regions Claude touches have behavioral descriptions, and that is exactly what workload-driven Phase 2 produces.

## Why now

Phase 2 (redirected) shipped a fast cold index and a workload-driven synthesis loop that gradually enriches the regions Claude touches with behavioral descriptions. `mycel find` today is one-shot vector search against those embeddings — solid baseline retrieval, but it leaves the structural half of the graph unused at query time. The headline Tier-4 pipeline in DESIGN.md is:

```
embed query
  → vector search (top 50)
  → graph expand 1–2 hops
  → rerank ~200 candidates against the query
  → return top 8–12
```

We have step 1 and a clipped step 2 (one-shot top-K instead of top-50 + expansion). Steps 3 (expansion) and 4 (rerank) are unbuilt. The trait surfaces are locked in — `Reranker` exists in `mycel-models` and `OllamaReranker::rerank` is a `todo!()` placeholder; per-tier reranker config slots already merge in `mycel-cli/src/config.rs`. The scaffolding is in place; Phase 3 fills it in.

Two things have changed since Phase 1 that bear on the design:

1. **Workload-driven synthesis means the graph is partially described, not uniformly described.** Mixed-state retrieval (some Symbols with behavioral descriptions, most with signature+body embeddings) is the *steady state*, not a transition. The Phase 3 reranker has to behave well in this regime — including not over-trusting cosine scores from heterogeneous embedding inputs.
2. **The structural blocker for graph expansion is now load-bearing.** CALLS / USES_TYPE / IMPLEMENTS edges are spotty: tree-sitter resolves same-file callees only, and the multilspy bridge currently emits degenerate REFERENCES (target is a file URI, not a Symbol qname). Phase 1 and Phase 2 didn't depend on those edges at query time. Phase 3 step 2 — "graph expand each candidate's 1–2 hop neighborhood" — does. We have to unblock cross-file edge resolution as part of Phase 3, or Tier 4 collapses back to "vector → rerank" and the headline thesis goes unproven.

## Scope

**In scope (Phase 3 ships):**

- **Workstream A — Cross-file edge resolution.** Replace the degenerate-REFERENCES path in `scripts/multilspy_bridge.py` with a call-site → definition resolver: tree-sitter's tentative CALLS edges feed call-site coordinates to the bridge; the bridge calls `request_definition` on each and emits an LSP-sourced CALLS edge with the target Symbol qname (resolved by mapping `(target_uri, target_line)` back to the enclosing Symbol). Same path lifts USES_TYPE and IMPLEMENTS to be cross-file-accurate where tree-sitter only had heuristics. This is the minimum to make graph-expand do real work in step 2.
- **Workstream B — Reranker wiring.** Implement `OllamaReranker::rerank` against the qwen3-reranker family using Ollama's `/api/generate` endpoint (qwen3-reranker is a yes/no-token scoring model — the score is the logit of the "yes" token given a `query: doc:` prompt template). Tier defaults: `qwen3-reranker:0.6b` on minimal/balanced, `qwen3-reranker:4b` on max. Batched per-candidate calls with bounded concurrency.
- **Workstream C — Full Tier 4 pipeline.** New module `mycel-query/src/find_tier4.rs` (the existing `find.rs` Phase 1 vector-only path stays in place as a fallback callable via `mycel find --no-rerank`). Pipeline:
  1. Embed the query.
  2. Vector search top 50.
  3. Graph-expand each candidate's 1-hop CALLS/CALLED_BY/USES_TYPE/IMPLEMENTS neighborhood (configurable depth, default 1; depth-2 is a future option once we measure cost).
  4. Dedupe to a candidate set capped at ~200.
  5. Rerank against the original query (with each candidate's description + signature + body slice as the document).
  6. Return top 8–12 (configurable; default 10).
- **Workstream D — Query-time on-demand synthesis** (the workload-driven Phase 2 thesis applied to retrieval). When the top-ranked candidates have NULL `synthesized_description` *and* the cosine scores cluster in a low-confidence band (operationalized as: top-3 cosine scores all below a tunable threshold *and* spread between top-1 and top-10 is narrow — meaning "vector search has no strong opinion"), synthesize behavioral descriptions for the borderline candidates inline, re-embed, and re-rank with the new vectors and descriptions. Bounded: at most N descriptions per query (default 5), with the per-query synthesis budget capped so a single bad query can't hijack the daemon's Synthesizer. Persisted to the graph via the existing atomic write — future queries reap the benefit.
- **Workstream E — Reranker eval harness.** New crate `mycel-eval` (or `crates/mycel-query/src/eval/` if a separate crate is overkill). Hand-curated corpus of ~30 Mycelium-shape queries with ground-truth Symbol qnames against this very repo (Mycelium is the canonical small dogfood codebase). Harness runs `find` with each candidate reranker, reports nDCG@10 / MRR / hit-rate@5 per candidate. Candidates: `qwen3-reranker:0.6b`, `qwen3-reranker:4b`, plus sidecar-only candidates (`gte-reranker-modernbert-base`, `mxbai-rerank-v2`, `jina-reranker-v2`) gated behind a `--with-sidecar-rerankers` flag so users without those sidecars running don't see test failures. Winner promoted to default if it beats the current default by a non-trivial margin on the harness corpus *and* doesn't regress latency past the 500ms target.

**Explicitly out of scope:**

- New query-surface CLI commands beyond what Tier 4 needs. Tier 3 (`similar`, `canonical`, `recent`, `dead`, `tests`) is Phase 4.
- Co-change edges and git-derived derived edges. Phase 4.
- Reranker fine-tuning on accumulated retrieval-quality traces. Phase 5 at earliest.
- Cloud reranker impls (Cohere Rerank, Voyage Rerank API). Phase 5.
- The general SKILL.md teaching `mycel find` to Claude. Phase 5.
- Personalization layer touching reranking weights. Phase 5.
- BM25 / hybrid retrieval. Open question; deferred unless eval harness shows vector-only retrieval is the bottleneck.
- HNSW low-k flakiness fix. Worked around in Phase 1 by defaulting `--limit` to 20; Phase 3 raises it further as needed but does not chase a FalkorDB fix.

## Architecture

Five mostly-independent workstreams. A, B, and E can ship in any order; C depends on B (it calls the reranker); D depends on C (it modifies the Tier-4 pipeline). The full-merge gate is the eval harness clearing the success criteria.

### Workstream A — Cross-file edge resolution

Today the bridge ([`scripts/multilspy_bridge.py:70`](../../../scripts/multilspy_bridge.py)) walks document symbols, calls `request_definition` on each name, and emits `REFERENCES` edges with `to: <target_uri>` — the target is a file URI string, not a Symbol qname. The indexer never resolves these to real Symbol nodes, so they don't power any query.

The fix is symmetric to what multilspy already supports. Instead of "for each documentSymbol, find what it references", we do "for each call site / type reference / implements-clause that tree-sitter flagged, ask LSP for the precise definition, and map the response back to a Symbol qname." Concretely:

1. Tree-sitter extraction (`mycel-extract`) emits tentative edges with call-site / reference-site coordinates. Today it produces a `from` qname and a heuristic `to` qname; we extend the wire format to also carry `from_line`, `from_col` (already known) and a `ref_kind` tag (`calls` / `uses_type` / `implements`).
2. The bridge accepts a new request op `resolve_refs_for_file` (alongside the existing `edges_for_file`). Body: `{"id", "language", "repo_root", "path", "sites": [{"line", "col", "ref_kind"}]}`. For each site, the bridge calls `request_definition(path, line, col)`, and for each returned definition location it emits `{"from_path": path, "from_line": line, "to_uri": target_uri, "to_line": target_start_line, "kind": <ref_kind>, "source": "lsp"}`.
3. The Rust side in `mycel-lsp` translates each `(to_uri, to_line)` to a Symbol qname by querying the graph: `MATCH (s:Symbol) WHERE s.file_path = $uri_to_path AND s.start_line <= $line AND s.end_line >= $line RETURN s.qualified_name`. If no Symbol contains the line (e.g., the definition is module-level not yet extracted), log at DEBUG and drop the edge — same posture as Phase 1.
4. The pipeline (`mycel-index/src/pipeline.rs`) replaces tree-sitter-only CALLS/USES_TYPE/IMPLEMENTS edges with their LSP-resolved versions when present, falling back to tree-sitter when LSP doesn't resolve (degraded but not broken — e.g., LSP server unreachable, language without LSP support).

The existing `EdgeSource::Lsp` vs `EdgeSource::TreeSitter` tagging stays; queries don't filter by source today, but future-us can ("explain why this edge exists" or "show only high-confidence edges").

Languages without LSP (other than TS/Rust) keep the tree-sitter-only path. The Phase 1 anti-pattern against silent error handling applies: bridge errors return WARN-logged partials, not silent empty edge sets.

### Workstream B — Reranker wiring

`OllamaReranker::rerank(query, candidates)` should return `Vec<f32>` of length `candidates.len()`. Implementation pattern matching what qwen3-reranker expects:

- Build a `/api/generate` request per (query, candidate) pair, with prompt template `Query: {query}\nDocument: {doc}\nRelevant (yes/no):`.
- `options.temperature = 0.0`, `stream: false`, and crucially `options.logits` or `raw: true` so we can read out the yes-token logit. (Ollama exposes per-token logprobs via `/api/generate` with `options.logits` on recent builds; we'll verify the exact field name against the installed Ollama version during implementation. If logprobs are unavailable, fall back to parsing the literal `yes`/`no` completion — coarser but functional.)
- Bound concurrency at `tier == Max ? 8 : 4` to match Ollama's `num_parallel` default.
- Return the raw "yes" logit (or a 1.0/0.0 fallback when reading the completion text). Tier-4 ranking only needs *relative* scores within a candidate set; the absolute scale doesn't matter.

Failure modes:

- Reranker unreachable → log WARN, fall through to cosine-score ordering. `mycel find` should never refuse to return results because the reranker is sick.
- Reranker returns garbage (non-finite score) for a single candidate → treat as `-inf` for ranking, log WARN.
- Reranker is slow → enforced timeout per batch (default 5s); on timeout, fall through to cosine ordering. The 500ms p50 latency target is for the happy path; we'd rather degrade gracefully than block.

Reranker provider config follows the existing `ProviderConfig::Ollama { endpoint, model, ... }` pattern. Tier defaults populate via the same `default_reranker_model(cfg.models.tier)` helper we add alongside the existing `default_synthesizer_model`.

### Workstream C — Full Tier 4 pipeline

New file `crates/mycel-query/src/find_tier4.rs`. Function signature:

```rust
pub async fn find_tier4(
    g: &GraphClient,
    embedder: &dyn Embedder,
    reranker: &dyn Reranker,
    synthesizer: Option<&dyn Synthesizer>, // workstream D wires this; pass None to disable
    query: &str,
    opts: FindOptions,
) -> Result<Vec<RankedHit>>;
```

`FindOptions` fields: `vector_top_k` (default 50), `expand_depth` (default 1), `rerank_candidate_cap` (default 200), `final_top_k` (default 10), `low_confidence_synth_budget` (default 5), `low_confidence_cosine_threshold` (default 0.55).

`RankedHit` carries: `Symbol`, `cosine_score`, `rerank_score`, `Vec<ExpansionPath>` (so callers see *why* the hit was promoted — "expanded from `Y` via CALLS"), and a `source` tag (`vector` | `expansion` | `query_time_synth`).

Pipeline steps in `find_tier4`:

1. **Embed query.** Single call to `embedder.embed(&[query])`.
2. **Vector top-K.** Existing `vector_search_top_k(query_vec, 50, ...)`. Returns 50 `(Symbol, cosine_score)`.
3. **Graph expand.** For each seed, run a Cypher query: `MATCH (s:Symbol {qualified_name: $qn})-[:CALLS|:CALLED_BY|:USES_TYPE|:IMPLEMENTS*1..1]-(n:Symbol) RETURN n.qualified_name, n.kind, ... LIMIT $expansion_fanout_cap`. Fanout cap defaults to 8 per seed to keep total candidates ≤ 200 even when a seed is a hub. (`:CALLED_BY` is just `:CALLS` in reverse; we'll write the query as `(s)-[:CALLS]-(n)` directional-agnostic.) Each expanded Symbol carries an `ExpansionPath { from_seed, edge_kinds }` for explainability.
4. **Dedupe.** Symbols already in the seed set keep their seed status; expansion duplicates are dropped. Hard cap at 200 candidates.
5. **Materialize documents.** For each candidate, build the rerank document: `description ?? signature` plus the first ~40 lines of body. Same body slice helper Phase 2 already exposes (`mycel-index/src/body_slice.rs`).
6. **Low-confidence check (workstream D — feature-flagged).** If `synthesizer.is_some()` and the top-3 cosine scores are all below `low_confidence_cosine_threshold` *and* (top-1 minus top-10) is below 0.05 (no strong winner), select up to `low_confidence_synth_budget` candidates with NULL descriptions and synthesize+embed inline. The newly-embedded vectors are re-cosine-scored against the query vector and merge into the candidate set. Persistence: descriptions written through the same atomic path as `mycel set-description` so future queries reap the benefit (and the daemon's staleness detector handles invalidation if those functions get edited later).
7. **Rerank.** Single `reranker.rerank(query, &docs)` call. Pair each score with its candidate. Sort descending by rerank score, break ties by cosine score, take top-K.
8. **Return** with all three score fields populated for transparency.

`mycel find` (CLI in `mycel-cli/src/main.rs`) dispatches to `find_tier4` by default and to the Phase 1 vector-only `find` when `--no-rerank` is passed. JSON output schema extended to carry the new score fields plus expansion paths (additive; old consumers keep parsing).

### Workstream D — Query-time on-demand synthesis

The bet: workload-driven synthesis is *good* but slow to converge in cold regions of the graph. When a user issues a behavioral query into a region Claude has never described, the cold-graph cosine scores are weak and the reranker's job is harder. We have a Synthesizer wired and a clear failure signal (low + flat cosine distribution); paying for synthesis at query time, only for the borderline candidates, only when cosine has no opinion, is exactly the "spend tokens where the user actually needs them" thesis applied to retrieval.

This is workstream C step 6. The reason it's a separate workstream is that the heuristic — what counts as "low confidence" — is empirical and we want the eval harness (workstream E) to give us a defensible threshold before this lands as default-on. Ship behind a config flag (`MYCEL_QUERY_TIME_SYNTH=on`/`off`, default `on` once threshold is set, `off` until then). The Synthesizer is per-tier as Phase 2 already wires.

The persistence side-effect matters: every query-time synth call is a write back to the graph, and a write means future queries against that region are faster *and* the daemon's staleness detector now tracks those descriptions. We are paying once for synthesis triggered by retrieval, getting both the immediate rerank lift and the persistent graph improvement.

### Workstream E — Reranker eval harness

Goal: settle the "which reranker default" question empirically and give Phase 3 work a regression suite. Not goal: build a general-purpose retrieval benchmark.

Corpus: ~30 hand-curated behavioral queries against this repo, each with one canonical Symbol target qname and 0–2 acceptable alternate targets. Stored as `crates/mycel-query/eval/corpus.toml` (so it's reviewable in git and survives between sessions). Examples:

```toml
[[query]]
text = "atomic write of description and embedding"
target = "mycel_graph::client::GraphClient::set_symbol_description_and_embedding"

[[query]]
text = "tier detection from system memory"
target = "mycel_cli::config::detect_tier"
```

Harness (`cargo run -p mycel-query --bin rerank-eval`):

1. Reset graph to a known cold-index state via a fixture (eval harness ships its own ephemeral FalkorDB on a non-default port).
2. For each (reranker, query) pair, run `find_tier4` and record top-10 results.
3. Compute nDCG@10, MRR, and hit-rate@5 per reranker, plus mean / p95 latency.
4. Print a comparison table and a pass/fail verdict against the success criteria.

Stretch: support `--with-sidecar-rerankers` to also benchmark `gte-reranker-modernbert-base`, `mxbai-rerank-v2`, `jina-reranker-v2` (the spec-listed candidates from DESIGN.md). Default off because those require sidecar containers most users won't have running.

CI: a `cargo test` integration test runs the harness in a "smoke" mode (3 queries instead of 30) against the bundled Ollama reranker only, so regressions get caught without paying the full eval cost on every PR.

## Data flow (full Tier 4, post-Phase-3)

```
user task in Claude Code
  → mycel find "validate OAuth bearer tokens for the /auth/me endpoint"
    → CLI loads config (resolves embedder, reranker, synthesizer from tier)
    → embedder.embed(query)                              ≈ 30ms
    → graph.vector_search_top_k(qv, 50)                  ≈ 20ms (FalkorDB HNSW)
    → for each seed: graph.expand_one_hop(qname, cap=8)  ≈ 30ms total (parallel)
    → dedupe to <=200 candidates
    → low-confidence check:
        if top3 < 0.55 && spread tight:
          synthesizer.synthesize(prompt) for up to 5 candidates  ≈ 200–500ms each (parallel, bounded)
          embedder.embed(new descriptions)
          graph.set_symbol_description_and_embedding(...)        (persisted!)
          re-cosine-score
    → reranker.rerank(query, docs)                       ≈ 100–300ms (parallel batches)
    → return top 10 with cosine_score, rerank_score, expansion_paths
  → Claude reads top hits, reasons, writes back descriptions via mycel-graph-care skill
  → graph keeps getting better, scoped to where the user actually works
```

Happy-path p50 latency target: ≤ 500ms (DESIGN.md). With workstream D firing on a cold region: ≤ 2s (one-time cost per region; subsequent queries hit the persisted descriptions). The p95 budget for query-time synthesis is the place we'll feel the tier divide most — `qwen3.6:35b-a3b` on max is fast enough to keep this comfortable; `gemma4:e2b` on minimal will be slower but still acceptable since the budget caps fan-out.

## Concurrency and ordering

Phase 2 already established the "last-writer-wins on description_source_hash" model. Phase 3 adds three new interleavings; all three resolve cleanly on top of the existing atomic-write guarantees.

| Sequence | Resulting state | Self-consistent? |
|----------|-----------------|------------------|
| Query-time synth writes description for `f` → user edits `f` → daemon clears description | Description cleared, signature+body embedding restored, body_hash updated | ✅ Same as Phase 2 |
| Two parallel `find` calls trigger query-time synth on the same Symbol | Both Cypher writes are atomic; last write wins on description. Rerank in each call uses the description that existed when it materialized docs (step 5) — one call may see the old, one may see the new. Rare in practice; acceptable. | ⚠ Content non-determinism across parallel calls; schema consistent. |
| User edits `f` mid-`find` → daemon clears `f`'s description → `find` reranks using the now-cleared description it read in step 5 | Rerank uses a description that no longer exists in the graph; result still returned but explains a state of the world that's no longer current. Cost: a slightly stale rank for one query. | ⚠ Same family as Phase 2's "Claude writes from stale memory" — accepted. |

Eval harness contention: harness runs against an isolated graph on a non-default port, so it can't race with the user's live daemon.

## Schema delta

No new node properties. Phase 2's `body_hash` and `description_source_hash` cover everything Phase 3 needs. Edge properties unchanged.

We *do* expect the indexed-edge population to change shape (cross-file CALLS / USES_TYPE / IMPLEMENTS rising dramatically once workstream A lands), but the schema accommodates that with no changes.

## Success criteria

Pre-merge gates:

1. **Cross-file edges land.** `mycel callers content_hash` returns ≥ 3 results on this repo (currently returns 0, grep finds 3). `mycel uses Symbol` returns ≥ 30 results (grep finds 67). `mycel implements Embedder` returns ≥ 1 (`OllamaEmbedder`). Codified as integration tests in `mycel-query`.
2. **Reranker integration works.** `OllamaReranker::rerank("validate auth", &["fn auth_check()", "fn render_button()"])` returns two finite floats with the first ≥ the second. Codified as a unit test gated on `MYCEL_OLLAMA_AVAILABLE=1`.
3. **Tier 4 end-to-end latency.** `mycel find` p50 ≤ 500ms on this repo, p95 ≤ 1s, on tier `balanced` hardware, with reranker on. Measured by the eval harness's latency report.
4. **Tier 4 quality.** Eval harness reports hit-rate@5 ≥ 80% against the curated corpus with reranker on, vs. the vector-only Phase 1 baseline. (If the cold-graph baseline is itself already ≥ 80%, the rerank lift will look small in absolute terms; we'll also report the relative lift.)
5. **Query-time synth quality.** For the subset of corpus queries that target cold-graph regions (no pre-existing descriptions in the relevant subgraph), enabling workstream D lifts hit-rate@5 by ≥ 10 absolute percentage points over D-disabled.

Post-ship observations (not blocking):

6. **Reranker pick.** Eval harness picks a default. If it's not `qwen3-reranker`, update DESIGN.md hardware-tiers table accordingly.
7. **Query-time synth coverage.** After a week of dogfood use, ≥ 20% of `mycel find` invocations on cold regions triggered query-time synth at least once.

## Testing

**Unit tests:**

- `OllamaReranker::rerank` happy path against a mocked Ollama HTTP server (no real network).
- `OllamaReranker::rerank` graceful degradation: timeout, 5xx, malformed response → cosine fallback exercised.
- Tier 4 candidate dedupe: a Symbol that appears both as a seed and as an expansion is kept once with its seed metadata.
- Low-confidence detector: parametrized over score distributions; assert it fires on flat/low and stays quiet on peaked/high.
- Expansion fanout cap: a Symbol with 100 callers contributes only 8 expansion candidates.

**Integration tests** (require ephemeral FalkorDB on `redis://127.0.0.1:16379`):

- Index this repo cold; assert cross-file CALLS edges land for at least 3 known cross-file call sites.
- End-to-end `find_tier4` against a small fixture graph; assert top-1 result matches the seeded target.
- Query-time synth persistence: run a query that triggers synth, verify the description is in the graph after the call returns, and a second invocation of the same query is faster and skips synth.
- Mixed-state graph: half the candidates have descriptions, half don't; assert rerank scores aren't catastrophically biased toward the described half. (Operationalized: top-10 contains at least 3 undescribed candidates when the ground truth includes both.)

**Eval-harness "smoke" test:** runs 3 corpus queries on every CI run; full 30-query harness runs nightly or on a manual `cargo run` invocation.

## Error handling

Phase 1 anti-pattern remains in force: **don't swallow errors**. New surfaces:

- Reranker unreachable → WARN, fall through to cosine. Surface in the JSON output (`reranker_degraded: true`) so callers know.
- Reranker timeout → WARN, fall through to cosine. Same JSON flag.
- Graph expansion fails on a single seed → log WARN, drop that seed's expansion, continue with the rest.
- Query-time synth fails on a single candidate → log WARN, leave that candidate's description NULL, continue. Do not abort the query.
- LSP bridge dies mid-extraction (workstream A) → existing `MycelError::Lsp("bridge died: ...")` path is correct; the indexer surfaces it loudly rather than silently shipping a graph with missing CALLS edges.

The reranker fallback path (cosine-only) is *not* a silent error suppression — it's a documented degraded mode signaled to the caller. The principle holds: surfacing degradation is fine; pretending it didn't happen is not.

## Migration

No schema migration. No reindex required for existing graphs. The cross-file edge fix produces new edges on the next incremental pass automatically; existing degenerate REFERENCES edges stay in the graph (harmless — nothing queries them) and naturally roll off as files are re-extracted. Users who want a clean slate run `mycel index --force-cold-rebuild`.

Reranker config slot already exists in `ProvidersOverlay` / `ProvidersConfig`. Workstream B adds `reranker_from_cfg(cfg)` mirroring the existing `embedder_from_cfg` and `synthesizer_from_cfg`. Existing user configs that don't mention `reranker` pick up the tier default automatically.

## Phase plan delta

- **Phase 3 (this design)** — cross-file edge resolution, reranker integration, full Tier 4, query-time on-demand synthesis, reranker eval harness.
- **Phase 4** — Tier 3 queries (`similar`, `canonical`, `recent`, `tests`, `dead`), CO_CHANGED and TESTED_BY edges from git history, task-knowledge retrieval logging.
- **Phase 5** — broader SKILL.md, plugin payload + marketplace listing, cloud provider impls (Anthropic / OpenAI / Cohere Rerank / Voyage Rerank), token measurement, convention/style/failure personalization layers.

## Open questions (deferred, not blocking)

- **Sidecar rerankers.** The DESIGN.md candidate list includes `gte-reranker-modernbert-base`, `mxbai-rerank-v2`, `jina-reranker-v2`, all of which need separate sidecar containers because Ollama can't serve them natively today. Worth running through the eval harness once, but unless one wins by a meaningful margin, the operational cost of a sidecar isn't justified for the default. Decision after harness results.
- **Reranker fine-tuning.** Whether the accumulated query/result trace (a Phase 4 deliverable) plus a small LoRA on `qwen3-reranker:0.6b` would materially beat off-the-shelf is an empirical question; left for Phase 5.
- **Expansion depth.** Depth-1 is the default; depth-2 could double-rank semi-relevant code at the cost of 5–10x more candidates per seed. Worth a harness sweep once depth-1 is stable.
- **Hybrid BM25 retrieval.** If the eval harness reveals that the vector stage is missing surface-syntactic matches (e.g., users searching for an exact function name), adding a BM25 stage upstream of vector search is a small change. Defer until evidence.
- **Reranker prompt template tuning.** qwen3-reranker is sensitive to the exact `Query: ... Document: ...` framing. The eval harness lets us sweep templates cheaply; we'll fix the default template once we've run the sweep.
- **Per-language LSP support for cross-file edges.** Workstream A unblocks TS and Rust (the only LSPs we currently spawn). Adding Python / Go later is more pipeline work but doesn't require Phase 3 design changes.

## How this fits into the Phase 2 workload-driven posture

Phase 2's bet was that *Claude itself* is the right Synthesizer for the regions Claude touches. Phase 3 extends that bet in two directions:

1. **Retrieval gets graph structure** — vector search alone discards the structural information the graph already encodes. Tier 4 pulls it in. This is orthogonal to the Phase 2 thesis; it's the other half of the Mycelium argument.
2. **Retrieval triggers more workload-driven synthesis** — query-time on-demand synthesis (workstream D) is the same workload-driven principle, but the trigger shifts from "Claude read this function" to "a user query landed in a cold region and we have no good way to rank it." Both triggers persist descriptions back to the graph, both compound over time, both are paid for at the moment of actual user value rather than upfront. The Phase 2 redirection laid the rails; Phase 3 runs another train on them.

Workstream A (cross-file edges) is the one piece that *doesn't* compose with the Phase 2 thesis directly — it's structural cleanup that should have been Phase 1 quality but was deferred because Phase 1 only used same-file edges. It blocks workstream C, so it lands in Phase 3 by necessity.
