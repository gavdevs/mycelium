# Mycelium — Claude Code instructions

Mycelium is a local-first, graph-aware code intelligence layer for AI agents. We are dogfooding it: the indexer watches *this* repo and you should use it for navigation.

## Use `mycel` instead of grep+Read for symbol/code lookup

The daemon is registered as a systemd-user service (`mycel.service`) and watches this tree with a 2s debounce. Edits you make are reflected in the graph within seconds.

**As of 2026-05-04 benchmarking: only `definers` is reliable.** Use it for "where is X defined" — it returns a line range and signature in one shot, saving the subsequent `Read`. Fall back to grep for everything else.

Run from the repo root (`target/release/mycel`; not on PATH):

| Task | Command | Status |
|------|---------|--------|
| Find a symbol by name | `target/release/mycel --repo . definers <name>` | **Works** — returns file/line-range/signature |
| Semantic exploration | `target/release/mycel --repo . find '<query>'` | **Unreliable** — signature-only embeddings cluster by name shape, not behavior; use grep instead |
| Callers/callees | `target/release/mycel --repo . callers <name>` | **Broken** — returns empty; use grep |
| Type usage | `target/release/mycel --repo . uses <type>` | **Broken** — returns empty; use grep |
| Interface implementations | `target/release/mycel --repo . implements <iface>` | **Broken** — IMPLEMENTS edges not landing; use grep |

Add `--json` for structured output. If you've rebuilt the daemon: `target/release/mycel daemon stop && target/release/mycel daemon start`.

## Phase 1 limitations to be aware of

These are tracked and being worked on; flag them when they bite, don't try to fix in passing:

- **`find` clusters by name shape, not behavior.** Benchmarked: "compute content hash for deduplication" returned `Indexer`, `Reranker`, `launchd` — the actual `content_hash` function wasn't in the top 8. Descriptions land in Phase 2; until then, use grep when `find` misses.
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
- **Phase 2** — synthesizer wired (gemma4:e2b/e4b/qwen3.6:35b-a3b), description generation with 1-hop graph context, re-embed on descriptions. Where retrieval quality jumps.
- **Phase 3** — reranker + full Tier 4 (vector → graph expand → rerank), reranker eval harness.
- **Phase 4** — git-derived edges (`CO_CHANGED`, `TESTED_BY`), Tier 3 queries.
- **Phase 5** — personalization layers, the SKILL.md, token measurement, cloud providers.

Each phase is additive — don't rewrite earlier phases. Cross-file edge resolution and LSP refinement quality are Phase 2/3 work even though they affect Phase 1 query usefulness.
