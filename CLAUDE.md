# Mycelium — Claude Code instructions

Mycelium is a local-first, graph-aware code intelligence layer for AI agents. We are dogfooding it: the indexer watches *this* repo and you should use it for navigation.

## Use `mycel` instead of grep+Read for symbol/code lookup

The daemon is registered as a systemd-user service (`mycel.service`) and watches this tree with a 2s debounce. Edits you make are reflected in the graph within seconds.

**As of 2026-05-04 benchmarking: only `definers` is reliable.** Use it for "where is X defined" — it returns a line range and signature in one shot, saving the subsequent `Read`. Fall back to grep for everything else.

Run from the repo root (`target/release/mycel`; not on PATH):

| Task | Command | Status |
|------|---------|--------|
| Find a symbol by name | `target/release/mycel --repo . definers <name>` | **Works** — returns file/line-range/signature |
| Semantic exploration | `target/release/mycel --repo . find '<query>'` | **Improving** — Phase 2 lands description-based embeddings; quality jumps as `mycel synthesize` runs across the graph |
| Run Phase 2 description synthesis | `target/release/mycel --repo . synthesize [--force] [--limit N]` | **Works** — synthesizes a behavioral description per Symbol via Ollama, then re-embeds. Idempotent. |
| Callers/callees | `target/release/mycel --repo . callers <name>` | **Broken** — returns empty; use grep |
| Type usage | `target/release/mycel --repo . uses <type>` | **Broken** — returns empty; use grep |
| Interface implementations | `target/release/mycel --repo . implements <iface>` | **Broken** — IMPLEMENTS edges not landing; use grep |

Add `--json` for structured output. If you've rebuilt the daemon: `target/release/mycel daemon stop && target/release/mycel daemon start`.

`mycel index` runs the description-synthesis pass automatically at the end on a configured synthesizer (`MYCEL_SYNTHESIZER=off` to disable, `--no-descriptions` flag for a fast cold reindex). The daemon's incremental path always skips synth — per-file edits don't pay an LLM call apiece. After daemon-driven reindexes, run `mycel synthesize` to refresh descriptions across the graph.

## Phase 1 / Phase 2 limitations to be aware of

These are tracked and being worked on; flag them when they bite, don't try to fix in passing:

- **`find` quality depends on description coverage.** Phase 2 ships description synthesis — a freshly indexed graph has signature-embedded symbols until `mycel synthesize` runs. Symbols with descriptions cluster by behavior; signature-embedded symbols cluster by name shape. Mixed states (partial coverage) give mixed results.
- **Module-declaration hallucinations.** Single-line `mod foo;` symbols get rich behavioral descriptions hallucinated from the module's name only (e.g., `mod launchd;` → "core logic for managing background services… launching system daemons"), which cluster against unrelated queries. Known follow-up: skip `SymbolKind::Module` in `list_symbols_for_synthesis`. Until then, filter top results by `kind` if a module decl is dragging your search off course.
- **HNSW low-k flakiness.** FalkorDB's HNSW vector index walks adaptively; at `--limit < 10` it sometimes returns zero rows on valid queries that have answers at `--limit 20`. Default is `20` post-Phase 2; raise it further if you suspect a result is being clipped.
- **`callers`, `uses`, `implements` all return empty.** Benchmarked: `callers content_hash` → empty (grep found 3 callers). `uses Symbol` → empty (grep found 67). `implements Embedder` → empty (OllamaEmbedder clearly implements it). CALLS, TYPED_BY, and IMPLEMENTS edges are not landing. Phase 2/3 work.
- **Cross-file CALLS edges drop silently.** Tree-sitter only resolves same-file callees; the multilspy bridge currently emits degenerate `REFERENCES` so cross-file CALLS don't land. Phase 2/3 territory.
- **`IMPORTS` queries return empty** — tree-sitter emits import edges as file→file, not Symbol→Symbol. Use `grep -rn 'use <crate>'` directly.

If a `mycel` command returns empty when you expect results, **first verify the daemon is healthy** before assuming the query is wrong:

```sh
mycel daemon status     # check service is running
tail ~/.cache/mycel/daemon.log
docker ps | grep falkordb   # FalkorDB must be up
```

## Architecture

Nine-crate Cargo workspace. Owners by responsibility (read DESIGN.md and `docs/superpowers/specs/2026-05-03-phase-1-infrastructure-design.md` for the full picture):

- `mycel-core` — domain types, no IO
- `mycel-graph` — sole owner of FalkorDB + Cypher
- `mycel-extract` — tree-sitter per-language extractors
- `mycel-lsp` — multilspy subprocess coordination
- `mycel-models` — Embedder/Synthesizer/Reranker traits + Ollama impls
- `mycel-index` — pipeline orchestration (parse → refine → upsert → embed)
- `mycel-query` — Tier 1–4 query implementations
- `mycel-cli` — `mycel` binary
- `mycel-daemon` — `mycel-daemon` binary (notify watcher)

Schema (Symbol/Edge node types, EdgeKind enum, manifest) lives in `mycel-core`. All FalkorDB operations are raw Cypher — no typed ORM. Migrations live in `mycel-graph/src/migrations.rs` and are idempotent.

## Workflow conventions

- **Build before claiming success:** `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings`. The clippy gate has caught real bugs (unused imports, wrong patterns) that compilation alone misses.
- **Lint and tests must pass before commit.** `cargo test --workspace` runs unit + integration tests; integration tests in `mycel-graph` need an ephemeral FalkorDB on `redis://127.0.0.1:16379` (start with `docker run -d --rm --name mycel-falkordb-test -p 127.0.0.1:16379:6379 falkordb/falkordb:v4.18.3`).
- **Daemon dogfood:** when you change daemon-side code, `cargo build --release -p mycel-daemon && mycel daemon stop && mycel daemon start`. Verify it picked up the new binary by checking the next `extracted file=...` log line.
- **Don't catch errors to make them go away.** The codebase already has spots where errors get swallowed (e.g., the previous "file deleted between event and read; skip"). When you find one, surface it — don't add another.
- **Tier 1 daemon = single repo per service.** `MYCEL_REPO` is required; the install command bakes a specific repo into the systemd unit. Multi-repo registration (`mycel daemon add <path>`) is a future feature.

## Phase plan (DESIGN.md is authoritative)

- **Phase 1 (v0)** — symbol graph, Tier 1 queries, signature-embedded `find`. **Shipped.**
- **Phase 2** — synthesizer wired (gemma4:e2b/e4b/qwen3.6:35b-a3b), description generation with 1-hop graph context, re-embed on descriptions. **Shipped 2026-05-04.** Run `mycel synthesize` to upgrade an existing graph from signature- to description-embeddings.
- **Phase 3** — reranker + full Tier 4 (vector → graph expand → rerank), reranker eval harness.
- **Phase 4** — git-derived edges (`CO_CHANGED`, `TESTED_BY`), Tier 3 queries.
- **Phase 5** — personalization layers, the SKILL.md, token measurement, cloud providers.

Each phase is additive — don't rewrite earlier phases. Cross-file edge resolution and LSP refinement quality are Phase 2/3 work even though they affect Phase 1 query usefulness.
