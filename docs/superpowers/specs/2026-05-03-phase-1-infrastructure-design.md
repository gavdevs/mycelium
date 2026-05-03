# Mycelium — Phase 1 Infrastructure & Build Plan

**Status**: Approved design as of 2026-05-03. Companion to `DESIGN.md`. Implementation-level spec for the v0 ship.

## Purpose

`DESIGN.md` is the product spec — what Mycelium is and how it works in steady state. This document is the **infrastructure and Phase 1 implementation spec** — the concrete decisions about how the project is laid out, what gets built first, what's deferred behind clean abstractions, and how an agent picks up the work.

## Scope of Phase 1

Phase 1 ships the v0 product thesis: a graph-aware code intelligence layer that already saves tokens. Specifically:

- Cargo workspace skeleton with all 9 crates created from day one (stubs where appropriate).
- FalkorDB as a local Docker container; data in a Docker volume.
- Tree-sitter + multilspy extraction for **TypeScript / TSX / JavaScript / JSX** and **Rust** (`.rs`). Two languages from day one — TypeScript for the canonical web codebase target, Rust because we're building Mycelium in Rust and dogfooding against the project's own source is the fastest, most realistic feedback loop.
- Full FalkorDB schema (all node and edge types from `DESIGN.md`) including the vector index.
- `Embedder` trait wired to Ollama, with `embeddinggemma` as the default model.
- `Synthesizer` and `Reranker` traits stubbed but not implemented.
- Indexing pipeline: parse (tree-sitter) → refine (multilspy) → graph upsert → embed (signature-only).
- CLI commands: `mycel index`, `mycel callers`, `mycel callees`, `mycel definers`, `mycel imports`, `mycel uses`, `mycel implements`, `mycel find` (vector-only — no graph expansion or rerank yet).
- Daemon: `notify`-based file watcher with 2s debounce, content-hash-keyed dedup, indexing pipeline runner.
- Hardware-aware bootstrap script (detects memory tier, pulls correct Ollama models).
- Tiered config (`MYCEL_TIER` resolves to embedder/reranker/synthesizer choices).
- Layered XDG-respecting configuration (defaults → user → repo → env).
- Justfile with common dev tasks.

**Explicitly out of scope for Phase 1** (deferred to later phases per `DESIGN.md`):

- Description synthesis (Phase 2)
- Re-embedding on descriptions (Phase 2)
- Graph expansion + reranking on `find` (Phase 3)
- Co-change / test reachability / Tier 3 queries (Phase 4)
- Personalization layers, skill doc, cloud providers (Phase 5)
- Repo-local distributable index format (deferred indefinitely; see Open Questions in `DESIGN.md`)

## Workspace structure

Cargo workspace at the repo root. Nine crates organized by responsibility, each with a single clear job. Crate boundaries match the future seams identified in `DESIGN.md` so subsequent phases slot in without restructuring.

```
mycelium/
├── Cargo.toml                  # workspace manifest, shared dep versions
├── Cargo.lock
├── DESIGN.md                   # product spec
├── README.md
├── Justfile
├── rust-toolchain.toml         # pinned MSRV
├── .gitignore
├── docker/
│   └── docker-compose.yml      # FalkorDB
├── scripts/
│   ├── bootstrap.sh            # idempotent dev setup
│   └── multilspy_bridge.py     # Python sidecar for LSP coordination
├── crates/
│   ├── mycel-core/             # types, errors, no IO
│   ├── mycel-graph/            # FalkorDB client + Cypher
│   ├── mycel-extract/          # tree-sitter wrappers + per-language extractors
│   ├── mycel-lsp/              # multilspy subprocess coordination
│   ├── mycel-models/           # ModelProvider traits + Ollama impl
│   ├── mycel-index/            # pipeline orchestration
│   ├── mycel-query/            # Tier 1-4 query implementations
│   ├── mycel-cli/              # `mycel` binary
│   └── mycel-daemon/           # `mycel-daemon` binary
├── docs/
│   └── superpowers/specs/      # design specs (this document)
└── tests/
    └── fixtures/               # TS files for extractor golden tests
```

### Crate responsibilities

**`mycel-core`** — Pure domain types. No IO, no async, no panics on bad input. Anything every other crate needs is defined here.
- Public types: `Symbol`, `SymbolKind`, `Edge`, `EdgeKind`, `EdgeSource` (`TreeSitter` | `Lsp`), `FileRecord`, `RepoId`, `QualifiedName`, `Signature`, `IndexManifest` (embedder identity, dimension, schema version).
- Public errors: `MycelError` enum via `thiserror`.
- Deps: `serde`, `serde_json`, `thiserror`, `camino` (UTF-8 paths), `time`.

**`mycel-graph`** — Sole owner of Cypher and FalkorDB. No other crate touches them.
- Public API: `GraphClient::connect`, `upsert_symbol`, `upsert_edge_batch`, `query_callers`, `query_callees`, `query_imports`, `query_uses`, `query_implements`, `query_definers`, `vector_search_top_k`, `read_manifest`, `write_manifest`.
- Schema migrations: an idempotent `migrations/` module that runs index/constraint creation on connect. Tracked via a small `MigrationRecord` graph node.
- Deps: `falkordb` (with `tokio` + `tracing` features), `tokio`, `tracing`, `mycel-core`.

**`mycel-extract`** — Tree-sitter integration, per-language extractors.
- Public API: `Extractor` trait with `fn extract(file: &Path, content: &str) -> Result<ExtractionOutput>`. `ExtractionOutput` is a pure-data struct of symbols + tentative edges, all `EdgeSource::TreeSitter`. A small `for_language(file: &Path) -> Option<Box<dyn Extractor>>` factory dispatches by file extension.
- Phase 1 implementations:
  - `TypeScriptExtractor` covers `.ts/.tsx/.js/.jsx` via `tree-sitter-typescript`.
  - `RustExtractor` covers `.rs` via `tree-sitter-rust`. Symbols include `fn`, `struct`, `enum`, `trait`, `impl`, `type`, `const`, `static`, `mod`. Edges include `CALLS`, `USES_TYPE`, `IMPLEMENTS` (impl blocks), `IMPORTS` (use statements), `RE_EXPORTS` (`pub use`).
- Deps: `tree-sitter`, `tree-sitter-typescript`, `tree-sitter-rust`, `mycel-core`, `tracing`.

**`mycel-lsp`** — Coordinates multilspy subprocess for LSP-augmented edge resolution.
- Public API: `Resolver` trait with `fn refine(file: &Path, extraction: &ExtractionOutput) -> Result<Vec<Edge>>`. The returned edges are `EdgeSource::Lsp`.
- Phase 1 implementation: `MultilspyResolver` spawns a long-running Python subprocess (`scripts/multilspy_bridge.py`), communicates via JSON-line protocol over stdin/stdout, multiplexes requests. Per-language config maps file extension → language server name.
- Phase 1 supports two languages: TypeScript (`tsserver`) and Rust (`rust-analyzer`). multilspy ships clients for both; we just configure them.
- The Python bridge script is part of the repo; the daemon runs it via the user's Python (verified at bootstrap time).
- Future: native Rust LSP client implementation lands behind the same `Resolver` trait. No callers change.
- Deps: `tokio`, `serde_json`, `tracing`, `mycel-core`.

**`mycel-models`** — Model provider abstraction layer.
- Public traits: `Embedder` (`async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>` plus `dimension()` and `identity()`), `Synthesizer` (`async fn synthesize(&self, prompt: &str) -> Result<String>`), `Reranker` (`async fn rerank(&self, query: &str, candidates: &[&str]) -> Result<Vec<f32>>`).
- Phase 1 implementation: `OllamaEmbedder` (functional). `OllamaSynthesizer` and `OllamaReranker` exist as trait impls but `todo!()` for now — the daemon doesn't call them in Phase 1.
- Provider config is loaded from `mycel-config` and matches the `[providers.embedder]` etc. shape in `DESIGN.md`.
- Deps: `reqwest`, `tokio`, `serde`, `serde_json`, `tracing`, `mycel-core`.

**`mycel-index`** — Indexing pipeline orchestration.
- Public API: `Indexer::new(graph, extract, lsp, models)`, `Indexer::index_file(path)`, `Indexer::index_repo(root)`.
- Phase 1 stages active: parse → refine → graph upsert → embed (signature-only).
- Phase 2-4 stages stubbed: synthesize, re-embed-on-description, derived edges (`CO_CHANGED`, `TESTED_BY`).
- Owns: content-hash dedup (skip unchanged files), batch sizing for embed calls, error handling and partial-failure recovery.
- Deps: `mycel-graph`, `mycel-extract`, `mycel-lsp`, `mycel-models`, `mycel-core`, `tokio`, `tracing`.

**`mycel-query`** — Implementations of all CLI query commands.
- Public API: one async function per command — `callers(graph, symbol)`, `callees(...)`, `find(graph, embedder, query, limit)`, etc.
- Phase 1 implementations real: all Tier 1 + signature-only `find` (vector search top-K, no graph expansion or rerank).
- Phase 3-4 stubs: `find` upgraded path (graph expansion + rerank), Tier 2/3 queries.
- Deps: `mycel-graph`, `mycel-models`, `mycel-core`.

**`mycel-cli`** — `mycel` binary. Entry point for agents.
- Subcommands: `index <path>`, `callers <symbol>`, `callees <symbol>`, `definers <symbol>`, `imports <file>`, `uses <type>`, `implements <interface>`, `find <query> [--limit N]`, `daemon` (manage daemon lifecycle), `pack`/`unpack` (deferred — Phase 5+), `config` (show/edit), `--json` everywhere.
- Output formatters: human-readable (default), JSON (`--json`).
- Reads config, opens FalkorDB connection, dispatches to `mycel-query`. No daemon dependency at query time.
- Deps: `clap` (with `derive`), `tokio`, `serde_json`, `mycel-graph`, `mycel-query`, `mycel-models`, `mycel-core`.

**`mycel-daemon`** — `mycel-daemon` binary. The long-running indexer.
- Owns: `notify` filesystem watcher (debounced 2s), per-repo registration (`mycel daemon add <path>`), index queue, graceful shutdown.
- Talks to `mycel-index` for actual indexing work. The daemon is mostly orchestration.
- Logs to `~/.cache/mycel/daemon.log` with rotation.
- Deps: `tokio`, `notify`, `tracing`, `tracing-subscriber`, `tracing-appender`, `mycel-index`, `mycel-graph`, `mycel-extract`, `mycel-lsp`, `mycel-models`, `mycel-core`.

### Workspace-level conventions

- Top-level `Cargo.toml` declares `[workspace.dependencies]` for all shared deps. Each crate's `Cargo.toml` references workspace deps (`tokio.workspace = true`).
- `rust-toolchain.toml` pins to a specific stable Rust version (1.85+ — current as of May 2026).
- All async code uses `tokio` (not `async-std`, not `smol`).
- All errors at trait boundaries are `Result<T, MycelError>`. `anyhow` only at binary entry points (`mycel-cli`, `mycel-daemon` `main.rs`).
- All logging uses `tracing` with structured fields. `RUST_LOG`-controllable.

## Infrastructure

### FalkorDB

Local Docker container, single-instance, data persisted in a named Docker volume. The compose file lives at `docker/docker-compose.yml`:

```yaml
services:
  falkordb:
    image: falkordb/falkordb:latest
    container_name: mycel-falkordb
    ports:
      - "6379:6379"
      - "3000:3000"  # browser UI for debugging
    volumes:
      - falkordb-data:/data
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 5s
      timeout: 3s
      retries: 5

volumes:
  falkordb-data:
```

Connection: `redis://localhost:6379`. Configurable via `[storage] falkordb_url` to support remote endpoints later.

### Ollama

Runs natively on the dev machine (not containerized — containerized Ollama on Mac loses GPU access via Apple Silicon). Bootstrap verifies `ollama` is on `PATH` and pulls the correct models per detected tier.

### multilspy

Python subprocess. Bootstrap verifies `python3` is available and runs `pip install --user multilspy` (or `pipx install multilspy` if available). The bridge script (`scripts/multilspy_bridge.py`) is part of the repo — the daemon spawns it with `python3 scripts/multilspy_bridge.py`.

The bridge protocol is JSON-line over stdin/stdout:

```
→ {"op": "edges_for_file", "path": "src/foo.ts", "id": 1}
← {"id": 1, "edges": [{"kind": "calls", "from": "...", "to": "...", "...": "..."}]}
```

The daemon multiplexes requests with `id` correlation; the bridge processes one request at a time per language (multilspy's own concurrency model).

### Bootstrap script

`scripts/bootstrap.sh` is idempotent — running twice does nothing harmful.

```bash
#!/usr/bin/env bash
set -euo pipefail

# 1. Verify prereqs
for tool in docker cargo ollama python3 pip just; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "Missing required tool: $tool" >&2; exit 1
  }
done

# Optional language-server tooling (warn, don't fail — multilspy can install on first use)
command -v rust-analyzer >/dev/null 2>&1 || \
  echo "warning: rust-analyzer not on PATH — Rust LSP refinement will be slower on first use" >&2

# tsserver ships inside the typescript npm package; multilspy manages it.

# 2. Detect memory tier
detect_memory_tier() {
  local total_gb
  total_gb=$(sysctl -n hw.memsize | awk '{printf "%.0f", $1/1024/1024/1024}')
  if [ "$total_gb" -lt 12 ]; then echo "minimal"
  elif [ "$total_gb" -lt 24 ]; then echo "balanced"
  else echo "max"; fi
}
TIER="${MYCEL_TIER:-$(detect_memory_tier)}"
echo "→ Detected tier: $TIER (override with MYCEL_TIER=...)"

# 3. Start FalkorDB
docker compose -f docker/docker-compose.yml up -d
until docker exec mycel-falkordb redis-cli ping >/dev/null 2>&1; do
  sleep 1
done
echo "→ FalkorDB healthy"

# 4. Pull tier-appropriate Ollama models
case "$TIER" in
  minimal)
    models=("embeddinggemma" "qwen3-reranker:0.6b") ;;
  balanced)
    models=("embeddinggemma" "qwen3-reranker:0.6b" "gemma4:e4b") ;;
  max)
    models=("embeddinggemma" "qwen3-reranker:4b" "qwen3.6:35b-a3b") ;;
esac
for m in "${models[@]}"; do
  ollama pull "$m"
done

# 5. Install multilspy
python3 -m pip install --user --upgrade multilspy

# 6. Build workspace
cargo build --workspace

echo "→ Bootstrap complete. Try: just up && mycel index ."
```

Phase 1's `embeddinggemma` is the only model the binary will actually call — the other tier downloads are pre-pulled so future phases don't pay a 30-minute first-run penalty.

### Configuration

Layered config, XDG-respecting:

1. **Compiled defaults** (in `mycel-core::config::defaults`).
2. **User-global**: `~/.config/mycel/config.toml`.
3. **Per-repo**: `<repo>/.mycel.toml`.
4. **Environment variables**: `MYCEL_*` — highest precedence.

Default `~/.config/mycel/config.toml`:

```toml
[models]
tier = "minimal"   # auto-detected by bootstrap; user overrides

[storage]
falkordb_url = "redis://localhost:6379"

[providers.embedder]
type = "ollama"
endpoint = "http://localhost:11434"
# model is resolved from tier; explicit override wins
# model = "embeddinggemma"
# dimension = 768

[providers.synthesizer]
type = "ollama"
endpoint = "http://localhost:11434"

[providers.reranker]
type = "ollama"
endpoint = "http://localhost:11434"

[lsp]
multilspy_path = "python3 scripts/multilspy_bridge.py"
languages = ["typescript", "rust"]
```

State directories (XDG):

- `~/.config/mycel/` — config files.
- `~/.local/share/mycel/<repo-id>/` — per-repo derived data (Phase 5+ conventions/style files).
- `~/.cache/mycel/` — daemon logs, content-hash maps, retrieval logs.

### Justfile

```just
default: build

build:
    cargo build --workspace

test:
    cargo test --workspace

up:
    docker compose -f docker/docker-compose.yml up -d

down:
    docker compose -f docker/docker-compose.yml down

bootstrap:
    ./scripts/bootstrap.sh

logs:
    docker compose -f docker/docker-compose.yml logs -f falkordb

reset-db:
    docker compose -f docker/docker-compose.yml down -v
    docker compose -f docker/docker-compose.yml up -d

fmt:
    cargo fmt --all

lint:
    cargo clippy --workspace --all-targets -- -D warnings
```

### Daemon supervisor (cross-platform)

The daemon must run as a long-lived background process across both macOS and Linux from day one — Mycelium's author works on both platforms and the dev experience needs to be uniform. v0 ships first-class supervisor integration:

**Commands:**
- `mycel daemon install` — auto-detects platform, writes the appropriate service file, registers it with the platform's supervisor.
- `mycel daemon uninstall` — reverses the install.
- `mycel daemon start` / `stop` / `restart` / `status` — wraps the platform-native commands so the user has one consistent interface.
- `mycel daemon logs [--follow]` — tails the daemon log regardless of platform.
- `mycel daemon run` — foreground mode for development; no supervisor involvement.

**macOS — launchd user agent.**
Plist written to `~/Library/LaunchAgents/com.gav.mycel.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.gav.mycel</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/mycel-daemon</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>/Users/USERNAME/.cache/mycel/daemon.log</string>
    <key>StandardErrorPath</key><string>/Users/USERNAME/.cache/mycel/daemon.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin</string>
    </dict>
</dict>
</plist>
```

`mycel daemon install` substitutes the binary path (resolved from `$PATH`) and the user's home directory at install time, then runs `launchctl load ~/Library/LaunchAgents/com.gav.mycel.plist`.

**Linux — systemd user service.**
Unit file written to `~/.config/systemd/user/mycel.service`:

```ini
[Unit]
Description=Mycelium code intelligence daemon
After=docker.service

[Service]
Type=simple
ExecStart=/usr/local/bin/mycel-daemon run
Restart=on-failure
RestartSec=5
StandardOutput=append:%h/.cache/mycel/daemon.log
StandardError=append:%h/.cache/mycel/daemon.log
Environment=PATH=/usr/local/bin:/usr/bin:/bin

[Install]
WantedBy=default.target
```

`mycel daemon install` substitutes the binary path, runs `systemctl --user daemon-reload`, then `systemctl --user enable --now mycel.service`. User-mode (not system-mode) so Mycelium runs under the user's UID without sudo.

**Platform detection.**
Compile-time via `cfg(target_os = ...)` for the supervisor module's branching, runtime defensive fallbacks if the binary is run on an unexpected platform (FreeBSD, Windows). Phase 1 supports macOS + Linux; Windows is deferred (`mycel daemon run` works there manually).

The supervisor templates live as resource strings inside `mycel-cli` (compiled in via `include_str!`) — no need for a separate templates directory. Substitutions are done with simple string replacement at install time (no template engine; the substitutions are tiny and stable).

## Data flow

### Indexing path (daemon)

```
File change event (notify)
  → Debounce 2s
  → Read file content + compute content_hash
  → Skip if hash matches stored hash
  → Tree-sitter extract (mycel-extract::TypeScriptExtractor)
       → Symbols + tentative edges (source: TreeSitter)
  → multilspy refine (mycel-lsp::MultilspyResolver)
       → Refined edges (source: Lsp), if language is configured for LSP
  → Graph upsert (mycel-graph)
       → File node, Symbol nodes, Edges
  → Embed signatures (mycel-models::OllamaEmbedder)
       → Batch of 32 → 768-dim vectors
       → Vector property on Symbol nodes
  → Update content_hash record
```

### Query path (CLI)

```
mycel callers <symbol>
  → mycel-cli parses args
  → Connects to FalkorDB directly (no daemon involvement)
  → mycel-query::callers runs Cypher query
  → Format result (human or --json)
  → Print
  → Exit

mycel find <query>
  → mycel-cli parses args
  → Connects to FalkorDB
  → Connects to Ollama for query embedding
  → mycel-query::find embeds query, runs vector_search_top_k
  → Format result
  → Print
```

## Testing strategy

- **Unit tests** in each crate's source files. `mycel-core` types have property tests via `proptest` where it makes sense (qualified names, edge validity).
- **Golden snapshot tests** in `mycel-extract`. Real TS/TSX files in `tests/fixtures/`. Each fixture has an expected `ExtractionOutput` JSON. `cargo insta` for snapshot management. Regressions fail fast.
- **Integration tests** in `mycel-graph`. Spin up an ephemeral FalkorDB container per test run (or per test file). Schema migrations validated. Round-trip upserts verified.
- **Daemon end-to-end tests** in `mycel-daemon/tests/`. Create a temp repo with a few TS files, start daemon, verify graph contents after index. Use multilspy in test mode.
- **CLI smoke tests** in `mycel-cli/tests/`. Run `mycel callers FooBar` against a fixture-indexed graph, snapshot the output.

CI matrix: `cargo test --workspace` on Linux + macOS, `cargo clippy -- -D warnings`, `cargo fmt --check`.

TDD encouraged especially in `mycel-extract` because regressions there silently corrupt the graph.

## Observability

- `tracing` everywhere with structured fields (`%symbol_id`, `?duration`, etc.).
- Daemon writes to `~/.cache/mycel/daemon.log` with `tracing-appender` rolling daily.
- CLI logs to stderr; respects `RUST_LOG`.
- Sensitive data (file contents, prompts to LLMs) logged at `TRACE` only — `INFO` and below stays clean.
- Span instrumentation around: per-file indexing, per-batch embedding, per-query Cypher execution.

## Sequencing within Phase 1

The work fans out from a thin walking skeleton. Suggested order for an agent run, each step independently verifiable:

1. **Workspace skeleton** — create all 9 crates as empty stubs, workspace `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, basic `README.md`. `cargo build --workspace` passes (compiles nothing real yet).
2. **`mycel-core` types** — `Symbol`, `Edge`, `EdgeKind`, `EdgeSource`, `MycelError`. Unit tests for serde round-trips.
3. **Docker + bootstrap** — `docker-compose.yml`, `scripts/bootstrap.sh` (without LSP install yet), `Justfile`. `just up` brings up FalkorDB; `just down` tears it down. Verifiable manually.
4. **`mycel-graph` skeleton** — `GraphClient::connect`, schema migrations (create indices for `Symbol(qualified_name)`, `File(path)`, vector index on `Symbol.embedding`). Integration test against ephemeral container.
5. **`mycel-extract::TypeScriptExtractor`** — extracts symbols + tree-sitter-source tentative edges from a TS file. Golden snapshot tests with 5-10 fixture files (real-world `.ts/.tsx/.js/.jsx`).
6. **`mycel-extract::RustExtractor`** — same shape for `.rs` files via `tree-sitter-rust`. Golden snapshot tests with fixture files including the project's own source as it grows. Independent of step 5; can run in parallel.
7. **`mycel-lsp` with multilspy** — Python bridge script, Rust subprocess management, request multiplexing, configured for both `tsserver` and `rust-analyzer`. End-to-end test: pass a TS file and a Rust file, verify both return LSP-source edges.
8. **`mycel-models::OllamaEmbedder`** — HTTP client to Ollama's `/api/embed`. Integration test against running Ollama (with `embeddinggemma` pulled).
9. **`mycel-index` Phase 1 pipeline** — orchestrates extract → refine → upsert → embed. Dispatch by file extension to the right `Extractor`. Integration test: index a fixture repo containing both TS and Rust files, query the resulting graph.
10. **`mycel-query` Tier 1 + signature `find`** — Cypher implementations for all Phase 1 commands.
11. **`mycel-cli`** — clap subcommands, output formatters, JSON schema for `--json`.
12. **`mycel-daemon`** — `notify` watcher, debounce, repo registration, runs `mycel-index::Indexer` on changes.
13. **Cross-platform supervisor** — `mycel daemon install/uninstall/start/stop/status/logs/run`. launchd plist for macOS; systemd user unit for Linux. Compile-time `cfg`-gated platform branches. Smoke-test on both platforms.
14. **End-to-end dogfood test** — bootstrap the Mycelium repo itself, run `mycel index .`, run CLI queries against the graph of our own source. Verify Rust extraction produces sensible call graphs. Index a small open-source TS repo as a second target. Verify token-savings hypothesis qualitatively.

Each step ends with a working artifact. The agent verifies via tests + manual smoke check before moving to the next.

## Open implementation questions

These are real questions the implementer will hit. Decisions can wait until they actually matter:

- **multilspy deployment.** `pip install --user` works but pollutes user Python. `pipx` is cleaner but requires another tool. `uv tool install multilspy` is a third option. Pick at bootstrap-time; document the rationale.
- **FalkorDB Rust client (`falkordb` crate) maturity for production use.** It's at v0.2.1 — should hold up but the integration tests in `mycel-graph` are the safety net.
- **Vector index migration on schema changes.** If we add a property to `Symbol` that affects the index, do we recompute or just re-add? Document the migration story.
- **Test fixture repo selection.** What's the smallest real TS codebase with enough symbol diversity to be a useful end-to-end fixture? Pick during step 5 of the sequencing.

## Definition of done for Phase 1

The Phase 1 ship is "done" when:

1. `just bootstrap && mycel index .` completes without error against the **Mycelium repo itself** (Rust dogfood) and against a **small open-source TS repo** (TypeScript target).
2. All Tier 1 queries return correct results on both indexed repos: `mycel callers <symbol>`, `mycel callees <symbol>`, `mycel imports <file>`, `mycel definers <symbol>`, `mycel uses <type>`, `mycel implements <interface>`.
3. `mycel find "thing that does X"` returns plausible signature-embedded matches in both repos.
4. `mycel daemon install` works on both **macOS (launchd)** and **Linux (systemd user)**; `mycel daemon start` brings the daemon up under the platform supervisor; `mycel daemon logs` tails output uniformly across platforms.
5. The daemon updates the graph within ~5 seconds of a file edit.
6. All commands support `--json` with documented schemas.
7. `cargo test --workspace` passes on macOS and Linux; `cargo clippy --workspace --all-targets -- -D warnings` passes; `cargo fmt --check` passes.
8. README documents the bootstrap flow, the tier system, the supervisor commands per platform, and the "what's deferred" surface.

That's the v0. Phase 2+ build on top.
