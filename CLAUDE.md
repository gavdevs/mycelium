# Mycelium — Claude Code instructions

Mycelium is a local-first, graph-aware code intelligence layer for AI agents. We are dogfooding it: the indexer watches *this* repo and you should use it for navigation.

## Use `mycel` instead of grep+Read for symbol/code lookup

The daemon is registered as a systemd-user service (`mycel.service`) and watches this tree with a 2s debounce. Edits you make are reflected in the graph within seconds.

**As of 2026-05-17 (Phase 3 Workstream A):** `definers`, `callers`, `uses`, and `implements` are all reliable for TypeScript and Rust within the indexed surface. `find` is reliable with description coverage. `IMPORTS` queries still need `grep` (file→file edge granularity).

Run from the repo root (`target/release/mycel`; not on PATH):

| Task | Command | Status |
|------|---------|--------|
| Find a symbol by name | `target/release/mycel --repo . definers <name>` | **Works** — returns file/line-range/signature |
| Semantic exploration | `target/release/mycel --repo . find '<query>'` | **Works baseline** — embeds signature+body slice on cold index; quality lifts further as the `mycel-graph-care` skill writes descriptions for symbols you read |
| Run bulk description synthesis (manual / non-Claude path) | `target/release/mycel --repo . synthesize [--force] [--limit N]` | **Works** — opt-in bulk pass via Ollama. Not run by `mycel index` after the 2026-05-05 redirection. |
| Read a Symbol's current description | `target/release/mycel --repo . describe <qname>` | **Works** |
| Write a behavioral description (workload-driven) | `target/release/mycel --repo . set-description --qname <qname> --description "<text>"` | **Works** — invoked by the `mycel-graph-care` skill |
| Install the graph-care skill into Claude Code | `target/release/mycel --repo . skill install` | **Works** |
| Backfill legacy description hashes | `target/release/mycel --repo . synthesize --refresh-hashes-only` | **Works** — one-shot for graphs predating 2026-05-05 |
| Re-index from scratch (drop legacy descriptions) | `target/release/mycel --repo . index . --force-cold-rebuild` | **Works** |
| Callers/callees | `target/release/mycel --repo . callers <name>` | **Works** — cross-file CALLS via LSP definition resolution (Phase 3 Workstream A) |
| Type usage | `target/release/mycel --repo . uses <type>` | **Works** — cross-file USES_TYPE via LSP definition resolution (Phase 3 Workstream A) |
| Interface implementations | `target/release/mycel --repo . implements <iface>` | **Works** — cross-file IMPLEMENTS via LSP definition resolution (Phase 3 Workstream A) |

Add `--json` for structured output. If you've rebuilt the daemon: `target/release/mycel daemon stop && target/release/mycel daemon start`.

`mycel index` does NOT run description synthesis (post-2026-05-05 redirection). Cold-index embeddings are computed on `signature + body[:60 lines]` — fast, no LLM round-trips. Behavioral descriptions are written by Claude Code itself when you've read and reasoned about a function, via `mycel set-description`, instructed by the shipped `mycel-graph-care` skill. The bulk `mycel synthesize` command remains as a manual fallback for non-Claude workflows. When the daemon's watcher detects an edit that changes a Symbol's body slice, the corresponding stored description (if any) is cleared automatically and the symbol is re-embedded on the new signature+body — Claude refills the description next time it reads and understands the function.

## Phase 1 / Phase 2 limitations to be aware of

These are tracked and being worked on; flag them when they bite, don't try to fix in passing:

- **`find` quality depends on description coverage.** Phase 2 ships description synthesis — a freshly indexed graph has signature+body-slice-embedded symbols until Claude (via the `mycel-graph-care` skill) writes behavioral descriptions for the symbols you actually work with — coverage grows with use, not with a one-shot bulk command. Symbols with descriptions cluster by behavior; signature-embedded symbols cluster by name shape. Mixed states (partial coverage) give mixed results.
- **Module-declaration hallucinations.** (Applies to the bulk `mycel synthesize` path; the workload-driven skill instructs Claude to skip module decls.) Single-line `mod foo;` symbols get rich behavioral descriptions hallucinated from the module's name only (e.g., `mod launchd;` → "core logic for managing background services… launching system daemons"), which cluster against unrelated queries. Known follow-up: skip `SymbolKind::Module` in `list_symbols_for_synthesis`. Until then, filter top results by `kind` if a module decl is dragging your search off course.
- **HNSW low-k flakiness.** FalkorDB's HNSW vector index walks adaptively; at `--limit < 10` it sometimes returns zero rows on valid queries that have answers at `--limit 20`. Default is `20` post-Phase 2; raise it further if you suspect a result is being clipped.
- **`IMPORTS` queries return empty** — tree-sitter emits import edges as file→file, not Symbol→Symbol. Use `grep -rn 'use <crate>'` directly.
- **LSP-resolved edges only land within the indexed surface.** Cross-file CALLS / USES_TYPE / IMPLEMENTS that resolve into stdlib, node_modules, or any path outside the repo are dropped (the bridge returns the file URI, `symbol_containing` returns None, the edge is skipped). Same-language source files inside the repo work. Phase 3 Workstream A.

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
- **Phase 2** — workload-driven description synthesis. Cold index embeds signature+body slice; behavioral descriptions are written by Claude via the `mycel-graph-care` skill (`set-description` CLI). Bulk `mycel synthesize` remains as opt-in fallback. **Shipped 2026-05-04**, redirected 2026-05-05 (see `docs/superpowers/specs/2026-05-05-workload-driven-synthesis-design.md`).
- **Phase 3** — reranker + full Tier 4 (vector → graph expand → rerank), reranker eval harness.
- **Phase 4** — git-derived edges (`CO_CHANGED`, `TESTED_BY`), Tier 3 queries.
- **Phase 5** — personalization layers, the SKILL.md, token measurement, cloud providers.

Each phase is additive — don't rewrite earlier phases. Cross-file edge resolution and LSP refinement quality are Phase 2/3 work even though they affect Phase 1 query usefulness.
