# Mycelium — Claude Code instructions

Mycelium is a local-first, graph-aware code intelligence layer for AI agents. We are dogfooding it: the indexer watches *this* repo and you should use it for navigation.

## Use `mycel` instead of grep+Read for symbol/code lookup

The daemon is registered as a systemd-user service (`mycel.service`) and watches this tree with a 2s debounce. Edits you make are reflected in the graph within seconds. Before reaching for `grep`/`rg`/`find`, try the corresponding `mycel` query — fewer tokens, structurally aware, gives you line ranges and signatures directly.

Run from the repo root:

| Task | Command | Notes |
|------|---------|-------|
| Find a symbol by name | `mycel --repo . definers <name>` | Matches short name OR full qname; returns file/line/signature |
| Semantic exploration | `mycel --repo . find '<natural-language query>'` | Vector search over signature embeddings |
| Same-file callers/callees | `mycel --repo . callers <name>` / `callees <name>` | Cross-file is still broken (see below) |
| Type usage | `mycel --repo . uses <type>` | |
| Interface implementations | `mycel --repo . implements <iface>` | |

Add `--json` for structured output. The release binary lives at `target/release/mycel`; if you've rebuilt the daemon you may also need `mycel daemon stop && mycel daemon start` to pick up the new binary.

## Phase 1 limitations to be aware of

These are tracked and being worked on; flag them when they bite, don't try to fix in passing:

- **`find` clusters by surface syntax, not behavior.** Embeddings are signature-only in v0; descriptions land in Phase 2. So "compute a content hash" may surface the `dedup` module before `content_hash` itself. When `find` is blunt, fall back to `definers` or grep — don't keep retrying.
- **Cross-file CALLS edges drop silently.** Tree-sitter only resolves same-file callees; the multilspy bridge currently emits degenerate `REFERENCES` (file URIs as targets) so cross-file CALLS don't land. `mycel callers <name>` returns same-file callers only. Phase 2/3 territory — don't paper over this with fragile heuristics.
- **`IMPORTS` queries usually return empty** because tree-sitter emits import edges as file→file or file→bare-path, not Symbol→Symbol. Use `grep -rn 'use <crate>'` or read the file directly until the schema gets reconciled.

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
