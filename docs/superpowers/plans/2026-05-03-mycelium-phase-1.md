# Mycelium Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Mycelium v0 — a local-first code intelligence service that indexes a Rust + TypeScript codebase into FalkorDB and exposes Tier 1 structural queries plus signature-embedded `mycel find` via a CLI invoked by AI coding agents.

**Architecture:** Cargo workspace with 9 crates. Long-running Rust daemon watches files and runs an indexing pipeline (tree-sitter parse → multilspy LSP refinement → FalkorDB graph upsert → EmbeddingGemma embed via Ollama). A separate `mycel` CLI reads FalkorDB directly for queries. Cross-platform daemon supervisor (launchd on macOS, systemd user on Linux). All model calls go through swappable trait abstractions (`Embedder` / `Synthesizer` / `Reranker`) with Ollama as the v0 default.

**Tech Stack:** Rust 1.85+ (workspace, tokio, tracing, clap, notify, tree-sitter, falkordb crate, reqwest), Python 3 (multilspy bridge subprocess), Docker (FalkorDB container via docker-compose), Ollama (local embeddinggemma model), `just` for dev tasks, `cargo insta` for snapshot tests.

**Specs referenced:**
- Product spec: `DESIGN.md`
- Phase 1 infra spec: `docs/superpowers/specs/2026-05-03-phase-1-infrastructure-design.md`

**Conventions used in this plan:**
- File paths are absolute relative to repo root.
- Test code is given in full where it's the test under TDD discipline.
- Cargo.toml dependency lists are given in full where new crates are created.
- Commit messages are given verbatim.
- Each `- [ ]` line is a single action ≤5 minutes.
- Tasks are grouped into chunks; chunks are independently reviewable.

---

## File Structure

Files created or modified during Phase 1, organized by crate. Each file has a single responsibility.

### Repo root

| File | Purpose |
|---|---|
| `Cargo.toml` | Workspace manifest. Lists members, shared dep versions in `[workspace.dependencies]`. |
| `Cargo.lock` | Generated. |
| `rust-toolchain.toml` | Pin Rust to 1.85 stable. |
| `.gitignore` | `target/`, `.env`, `*.swp`, `.DS_Store`, `.mycel/` (not used in Phase 1 but reserved). |
| `Justfile` | Dev tasks: `up`, `down`, `bootstrap`, `build`, `test`, `fmt`, `lint`, `logs`, `reset-db`. |
| `README.md` | Project overview, prerequisite list, bootstrap quickstart, supervisor commands. |
| `docker/docker-compose.yml` | FalkorDB single-container service definition. |
| `scripts/bootstrap.sh` | Idempotent dev environment setup (prereq check, tier detection, model pull, multilspy install, build). |
| `scripts/multilspy_bridge.py` | Python long-running subprocess: JSON-line protocol bridging multilspy to mycel-daemon. |
| `tests/fixtures/typescript/*.ts*` | Golden-snapshot fixture TS files for mycel-extract tests. |
| `tests/fixtures/rust/*.rs` | Golden-snapshot fixture Rust files for mycel-extract tests. |

### `crates/mycel-core`

| File | Purpose |
|---|---|
| `Cargo.toml` | Manifest. Deps: serde, serde_json, thiserror, camino, time. |
| `src/lib.rs` | Re-exports public types. |
| `src/symbol.rs` | `Symbol`, `SymbolKind`, `QualifiedName`, `Signature`. |
| `src/edge.rs` | `Edge`, `EdgeKind`, `EdgeSource`. |
| `src/file.rs` | `FileRecord`, `RepoId`, content-hash type. |
| `src/manifest.rs` | `IndexManifest` (embedder identity + dimension + schema version). |
| `src/error.rs` | `MycelError` enum with thiserror. |
| `src/config.rs` | Config types (no IO — pure data); `Tier`, `ProvidersConfig`, `LspConfig`, etc. |

### `crates/mycel-graph`

| File | Purpose |
|---|---|
| `Cargo.toml` | Manifest. Deps: falkordb (tokio + tracing features), tokio, tracing, mycel-core. |
| `src/lib.rs` | Re-exports public surface. |
| `src/client.rs` | `GraphClient::connect`. Connection pool, retry. |
| `src/schema.rs` | Cypher constants for node + edge upsert/query. |
| `src/migrations.rs` | `MigrationRecord`-based migration runner. |
| `src/manifest.rs` | `read_manifest` / `write_manifest` against `mycel:meta`. |
| `src/symbol.rs` | `upsert_symbol`, `upsert_symbol_batch`. |
| `src/edge.rs` | `upsert_edge_batch`. |
| `src/queries.rs` | `query_callers`, `query_callees`, `query_imports`, `query_uses`, `query_implements`, `query_definers`. |
| `src/vector.rs` | `vector_search_top_k` (raw Cypher CALL `db.idx.vector.queryNodes`). |
| `tests/integration.rs` | Integration tests against ephemeral FalkorDB. |

### `crates/mycel-extract`

| File | Purpose |
|---|---|
| `Cargo.toml` | Manifest. Deps: tree-sitter, tree-sitter-typescript, tree-sitter-rust, mycel-core, tracing. |
| `src/lib.rs` | `Extractor` trait, `ExtractionOutput`, `for_language` factory. |
| `src/languages/typescript.rs` | `TypeScriptExtractor`. |
| `src/languages/rust.rs` | `RustExtractor`. |
| `src/queries.rs` | Tree-sitter query strings, organized by language. |
| `tests/typescript_fixtures.rs` | Golden-snapshot tests for TS extractor. |
| `tests/rust_fixtures.rs` | Golden-snapshot tests for Rust extractor. |

### `crates/mycel-lsp`

| File | Purpose |
|---|---|
| `Cargo.toml` | Manifest. Deps: tokio, serde_json, tracing, mycel-core. |
| `src/lib.rs` | `Resolver` trait. |
| `src/multilspy.rs` | `MultilspyResolver` — manages Python subprocess, request multiplexing. |
| `src/protocol.rs` | JSON-line request/response types. |

### `crates/mycel-models`

| File | Purpose |
|---|---|
| `Cargo.toml` | Manifest. Deps: reqwest, tokio, serde, serde_json, tracing, mycel-core. |
| `src/lib.rs` | `Embedder`, `Synthesizer`, `Reranker` traits. |
| `src/ollama.rs` | `OllamaEmbedder` (functional). `OllamaSynthesizer`/`OllamaReranker` stubs (`todo!()`). |
| `src/registry.rs` | `ModelRegistry` — resolves config to providers. |

### `crates/mycel-index`

| File | Purpose |
|---|---|
| `Cargo.toml` | Manifest. Deps: mycel-graph, mycel-extract, mycel-lsp, mycel-models, mycel-core, tokio, tracing, blake3. |
| `src/lib.rs` | `Indexer` struct + `index_file` / `index_repo` methods. |
| `src/pipeline.rs` | Pipeline stages composition. |
| `src/dedup.rs` | Content-hash dedup (queries `File.content_hash` from graph). |

### `crates/mycel-query`

| File | Purpose |
|---|---|
| `Cargo.toml` | Manifest. Deps: mycel-graph, mycel-models, mycel-core, tokio. |
| `src/lib.rs` | One async fn per CLI command. |
| `src/find.rs` | Phase 1 vector-only `find` implementation. |

### `crates/mycel-cli`

| File | Purpose |
|---|---|
| `Cargo.toml` | Manifest. Bin target. Deps: clap (derive), tokio, serde_json, mycel-graph, mycel-query, mycel-models, mycel-core, anyhow, tracing-subscriber. |
| `src/main.rs` | clap parser, dispatch, output formatting. |
| `src/output.rs` | Human + JSON formatters. |
| `src/config.rs` | Layered config loading (defaults → user → repo → env). |
| `src/supervisor/mod.rs` | Cross-platform supervisor entry point. |
| `src/supervisor/launchd.rs` | macOS supervisor (cfg-gated). Includes `launchd.plist.tmpl` via `include_str!`. |
| `src/supervisor/systemd.rs` | Linux supervisor (cfg-gated). Includes `mycel.service.tmpl` via `include_str!`. |
| `src/supervisor/launchd.plist.tmpl` | launchd plist template. |
| `src/supervisor/mycel.service.tmpl` | systemd unit template. |

### `crates/mycel-daemon`

| File | Purpose |
|---|---|
| `Cargo.toml` | Manifest. Bin target. Deps: tokio, notify, tokio-stream, futures, tracing, tracing-subscriber, tracing-appender, mycel-index, mycel-graph, mycel-extract, mycel-lsp, mycel-models, mycel-core, anyhow. |
| `src/main.rs` | Entry point; loads config, starts watcher, runs Indexer on events. |
| `src/watcher.rs` | `notify`-based filesystem watcher with 2s debounce. |
| `src/queue.rs` | Index queue with backpressure. |

---

## Chunk 1: Foundation (workspace + infra)

This chunk produces a buildable empty workspace, a running FalkorDB container, and an idempotent bootstrap script. By the end, `just bootstrap` works end-to-end on both macOS and Linux. No business logic yet.

### Task 1.1: Create the Cargo workspace skeleton

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `.gitignore`
- Create: `crates/mycel-core/Cargo.toml`
- Create: `crates/mycel-core/src/lib.rs`
- Create: `crates/mycel-graph/Cargo.toml`
- Create: `crates/mycel-graph/src/lib.rs`
- Create: `crates/mycel-extract/Cargo.toml`
- Create: `crates/mycel-extract/src/lib.rs`
- Create: `crates/mycel-lsp/Cargo.toml`
- Create: `crates/mycel-lsp/src/lib.rs`
- Create: `crates/mycel-models/Cargo.toml`
- Create: `crates/mycel-models/src/lib.rs`
- Create: `crates/mycel-index/Cargo.toml`
- Create: `crates/mycel-index/src/lib.rs`
- Create: `crates/mycel-query/Cargo.toml`
- Create: `crates/mycel-query/src/lib.rs`
- Create: `crates/mycel-cli/Cargo.toml`
- Create: `crates/mycel-cli/src/main.rs`
- Create: `crates/mycel-daemon/Cargo.toml`
- Create: `crates/mycel-daemon/src/main.rs`

- [ ] **Step 1: Write workspace `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = [
    "crates/mycel-core",
    "crates/mycel-graph",
    "crates/mycel-extract",
    "crates/mycel-lsp",
    "crates/mycel-models",
    "crates/mycel-index",
    "crates/mycel-query",
    "crates/mycel-cli",
    "crates/mycel-daemon",
]

[workspace.package]
version = "0.0.1"
edition = "2024"
license = "AGPL-3.0-only"
authors = ["Gav <gavdevs>"]
repository = "https://github.com/gavdevs/mycelium"
rust-version = "1.85"

[workspace.dependencies]
# async
tokio = { version = "1", features = ["full"] }
tokio-stream = "0.1"
futures = "0.3"
# error / log
thiserror = "2"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
tracing-appender = "0.2"
# serde
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.9"
# fs / paths
camino = { version = "1", features = ["serde1"] }
notify = { version = "8", features = ["macos_fsevent"] }
# storage
falkordb = { version = "0.2", features = ["tokio", "tracing"] }
# parsing
tree-sitter = "0.25"
tree-sitter-typescript = "0.23"
tree-sitter-rust = "0.24"
# http
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
# cli
clap = { version = "4", features = ["derive"] }
# misc
blake3 = "1"
time = { version = "0.3", features = ["serde", "formatting"] }
# test
insta = { version = "1", features = ["json", "yaml"] }
proptest = "1"

[profile.release]
lto = "thin"
codegen-units = 1
strip = "debuginfo"
```

Note on dep versions: these are best-current as of May 2026. If `cargo build` reports an unresolvable version, use `cargo search <crate>` to find the latest published; do not loosen with wildcards.

- [ ] **Step 2: Write `rust-toolchain.toml`**

```toml
[toolchain]
channel = "1.85.0"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

- [ ] **Step 3: Write `.gitignore`**

```
/target
**/*.swp
.DS_Store
.env
.env.local
.mycel/
*.log
```

- [ ] **Step 4: For each of the 9 crates, write a minimal `Cargo.toml` and stub `src/lib.rs` (or `src/main.rs` for cli/daemon)**

Pattern for library crates (`mycel-core`, `mycel-graph`, etc.):

```toml
# crates/<name>/Cargo.toml
[package]
name = "<name>"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
rust-version.workspace = true

[dependencies]
# (fill per crate; see "File Structure" section above)
```

`src/lib.rs` content for each library crate (placeholder):

```rust
//! <crate name>
//!
//! Phase 1 stub — populated by subsequent tasks.
```

For binary crates (`mycel-cli`, `mycel-daemon`), additionally:

```toml
[[bin]]
name = "<binary-name>"  # mycel for cli, mycel-daemon for daemon
path = "src/main.rs"
```

`src/main.rs` content for binary crates (placeholder):

```rust
fn main() {
    println!("Phase 1 stub — populated by subsequent tasks.");
}
```

For each crate, list its Cargo.toml dependencies per the "File Structure" section. Concrete examples:

`crates/mycel-core/Cargo.toml` deps:

```toml
[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
camino = { workspace = true }
time = { workspace = true }
```

`crates/mycel-graph/Cargo.toml` deps:

```toml
[dependencies]
falkordb = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
mycel-core = { path = "../mycel-core" }
```

`crates/mycel-extract/Cargo.toml` deps:

```toml
[dependencies]
tree-sitter = { workspace = true }
tree-sitter-typescript = { workspace = true }
tree-sitter-rust = { workspace = true }
mycel-core = { path = "../mycel-core" }
tracing = { workspace = true }
```

`crates/mycel-lsp/Cargo.toml` deps:

```toml
[dependencies]
tokio = { workspace = true }
serde_json = { workspace = true }
serde = { workspace = true }
tracing = { workspace = true }
mycel-core = { path = "../mycel-core" }
```

`crates/mycel-models/Cargo.toml` deps:

```toml
[dependencies]
reqwest = { workspace = true }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
mycel-core = { path = "../mycel-core" }
```

`crates/mycel-index/Cargo.toml` deps:

```toml
[dependencies]
mycel-graph = { path = "../mycel-graph" }
mycel-extract = { path = "../mycel-extract" }
mycel-lsp = { path = "../mycel-lsp" }
mycel-models = { path = "../mycel-models" }
mycel-core = { path = "../mycel-core" }
tokio = { workspace = true }
tracing = { workspace = true }
blake3 = { workspace = true }
camino = { workspace = true }
```

`crates/mycel-query/Cargo.toml` deps:

```toml
[dependencies]
mycel-graph = { path = "../mycel-graph" }
mycel-models = { path = "../mycel-models" }
mycel-core = { path = "../mycel-core" }
tokio = { workspace = true }
tracing = { workspace = true }
```

`crates/mycel-cli/Cargo.toml` (binary):

```toml
[[bin]]
name = "mycel"
path = "src/main.rs"

[dependencies]
clap = { workspace = true }
tokio = { workspace = true }
serde_json = { workspace = true }
serde = { workspace = true }
toml = { workspace = true }
mycel-graph = { path = "../mycel-graph" }
mycel-query = { path = "../mycel-query" }
mycel-models = { path = "../mycel-models" }
mycel-index = { path = "../mycel-index" }
mycel-lsp = { path = "../mycel-lsp" }
mycel-core = { path = "../mycel-core" }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
camino = { workspace = true }
blake3 = { workspace = true }
dirs = "5"
```

`crates/mycel-daemon/Cargo.toml` (binary):

```toml
[[bin]]
name = "mycel-daemon"
path = "src/main.rs"

[dependencies]
tokio = { workspace = true }
tokio-stream = { workspace = true }
futures = { workspace = true }
notify = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
tracing-appender = { workspace = true }
mycel-index = { path = "../mycel-index" }
mycel-graph = { path = "../mycel-graph" }
mycel-extract = { path = "../mycel-extract" }
mycel-lsp = { path = "../mycel-lsp" }
mycel-models = { path = "../mycel-models" }
mycel-core = { path = "../mycel-core" }
anyhow = { workspace = true }
camino = { workspace = true }
```

- [ ] **Step 5: Run `cargo build --workspace` to verify everything compiles**

Run: `cargo build --workspace`
Expected: clean build, all 9 crates compile (the binaries print the placeholder line; the libraries are empty).

If `falkordb` or any other dep fails to resolve at the listed version, run `cargo search <crate>` to find the latest 0.x.y or 1.x.y compatible release and update the workspace dep accordingly.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml .gitignore crates
git commit -m "Scaffold Cargo workspace with 9 stub crates"
```

---

### Task 1.2: Docker compose for FalkorDB and Justfile

**Files:**
- Create: `docker/docker-compose.yml`
- Create: `Justfile`

- [ ] **Step 1: Write `docker/docker-compose.yml`**

```yaml
services:
  falkordb:
    image: falkordb/falkordb:latest
    container_name: mycel-falkordb
    ports:
      - "6379:6379"
      - "3000:3000"
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

- [ ] **Step 2: Write `Justfile`**

```just
default: build

build:
    cargo build --workspace

test:
    cargo test --workspace --all-targets

up:
    docker compose -f docker/docker-compose.yml up -d

down:
    docker compose -f docker/docker-compose.yml down

bootstrap:
    bash scripts/bootstrap.sh

logs:
    docker compose -f docker/docker-compose.yml logs -f falkordb

reset-db:
    docker compose -f docker/docker-compose.yml down -v
    docker compose -f docker/docker-compose.yml up -d

fmt:
    cargo fmt --all

lint:
    cargo clippy --workspace --all-targets -- -D warnings

fmt-check:
    cargo fmt --all -- --check
```

- [ ] **Step 3: Smoke test — bring FalkorDB up and ping it**

Run: `just up`
Expected: container starts, no errors.

Then run: `docker exec mycel-falkordb redis-cli ping`
Expected: `PONG`.

Then: `just down`
Expected: clean shutdown.

- [ ] **Step 4: Commit**

```bash
git add docker Justfile
git commit -m "Add docker-compose for FalkorDB and Justfile dev tasks"
```

---

### Task 1.3: Bootstrap script with hardware-tier detection

**Files:**
- Create: `scripts/bootstrap.sh`

- [ ] **Step 1: Write `scripts/bootstrap.sh`**

```bash
#!/usr/bin/env bash
set -euo pipefail

# 1. Verify required tools
missing=()
for tool in docker cargo ollama python3 pip just; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing+=("$tool")
  fi
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "Missing required tools: ${missing[*]}" >&2
  echo "Install them and re-run." >&2
  exit 1
fi

# Optional language-server tooling — warn, don't fail
if ! command -v rust-analyzer >/dev/null 2>&1; then
  echo "warning: rust-analyzer not on PATH — Rust LSP refinement will be slower on first use" >&2
fi

# 2. Detect memory tier (cross-platform)
detect_memory_tier() {
  local total_bytes
  case "$(uname -s)" in
    Darwin) total_bytes=$(sysctl -n hw.memsize) ;;
    Linux)  total_bytes=$(awk '/MemTotal/ {print $2 * 1024}' /proc/meminfo) ;;
    *)      total_bytes=8589934592 ;;  # 8GB default
  esac
  local total_gb=$((total_bytes / 1024 / 1024 / 1024))
  if [ "$total_gb" -lt 12 ]; then echo "minimal"
  elif [ "$total_gb" -lt 24 ]; then echo "balanced"
  else echo "max"; fi
}
TIER="${MYCEL_TIER:-$(detect_memory_tier)}"
echo "→ Detected tier: $TIER (override with MYCEL_TIER=...)"

# 3. Start FalkorDB
echo "→ Starting FalkorDB..."
docker compose -f docker/docker-compose.yml up -d
echo "→ Waiting for FalkorDB to become healthy..."
healthy=0
for _ in $(seq 1 30); do
  if docker exec mycel-falkordb redis-cli ping >/dev/null 2>&1; then
    echo "→ FalkorDB healthy"
    healthy=1
    break
  fi
  sleep 1
done
if [ "$healthy" != "1" ]; then
  echo "FalkorDB did not become healthy after 30s; check 'just logs'" >&2
  exit 1
fi

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
  echo "→ Pulling Ollama model: $m"
  ollama pull "$m"
done

# 5. Install multilspy (user-scoped)
echo "→ Installing multilspy..."
python3 -m pip install --user --upgrade multilspy

# 6. Build the workspace
echo "→ Building workspace..."
cargo build --workspace

echo
echo "→ Bootstrap complete."
echo "  Try: just up && cargo run -p mycel-cli -- index <path>"
```

- [ ] **Step 2: Make it executable**

Run: `chmod +x scripts/bootstrap.sh`

- [ ] **Step 3: Smoke test — run bootstrap on the dev machine**

Run: `just bootstrap`
Expected: completes without error. May take 15+ minutes the first time due to model pulls.

If the user is running this manually for the first time and wants to skip model pulls, they can set `MYCEL_TIER=` to a no-op profile temporarily — but the standard path is to let it run.

- [ ] **Step 4: Commit**

```bash
git add scripts/bootstrap.sh
git commit -m "Add hardware-aware bootstrap script with tier detection"
```

---

### Task 1.4: README + project metadata

**Files:**
- Create: `README.md`

- [ ] **Step 1: Write `README.md`**

```markdown
# Mycelium

A local-first, graph-aware code intelligence layer for AI coding agents.

See `DESIGN.md` for the product spec and `docs/superpowers/specs/2026-05-03-phase-1-infrastructure-design.md` for the Phase 1 implementation spec.

## Prerequisites

Required:
- Rust 1.85+ (`rustup install 1.85`)
- Docker
- Ollama (`brew install ollama` or platform equivalent)
- Python 3.10+ with pip
- `just` (`brew install just` or `cargo install just`)

Recommended:
- `rust-analyzer` on PATH (faster Rust LSP refinement on first use)
- A TypeScript project to point Mycelium at (any reasonably-sized repo)

## Quickstart

```sh
just bootstrap   # detects memory tier, starts FalkorDB, pulls Ollama models, installs multilspy, builds
just up          # if you skipped bootstrap and just need FalkorDB running
cargo run -p mycel-cli -- index ./path/to/repo
cargo run -p mycel-cli -- find "the thing that does X"
```

## Hardware tiers

Bootstrap auto-detects:
- `minimal` (≤12GB RAM): embeddinggemma + qwen3-reranker:0.6b. No synthesis (Phase 2+).
- `balanced` (12–24GB RAM): + gemma4:e4b synthesizer.
- `max` (≥24GB RAM): + qwen3-reranker:4b + qwen3.6:35b-a3b synthesizer.

Override with `MYCEL_TIER=minimal|balanced|max`.

## Daemon

The daemon watches registered repos and re-indexes incrementally.

```sh
mycel daemon install   # registers with launchd (macOS) or systemd-user (Linux)
mycel daemon start
mycel daemon status
mycel daemon logs --follow
mycel daemon uninstall
```

For development without supervisor:

```sh
mycel daemon run   # foreground; ctrl-C to stop
```

## What's in v0

- Tree-sitter + multilspy extraction for TypeScript and Rust.
- FalkorDB graph with all node + edge types from DESIGN.md.
- Tier 1 queries: `callers`, `callees`, `definers`, `imports`, `uses`, `implements`.
- Signature-embedded `mycel find <query>` with vector search.
- Cross-platform daemon supervisor (launchd + systemd-user).

## What's deferred (later phases)

- Description synthesis (Phase 2).
- Graph expansion + reranking on `find` (Phase 3).
- Co-change / test reachability / Tier 3 queries (Phase 4).
- Personalization layers, skill doc, cloud providers (Phase 5).

## License

AGPL-3.0 (matching FalkorDB community license).
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "Add README with quickstart and tier documentation"
```

---

## Chunk 2: Core types + graph layer

This chunk produces `mycel-core` (pure types, no IO) and `mycel-graph` (the only crate that touches FalkorDB / Cypher). By the end, integration tests upsert symbols and edges into an ephemeral FalkorDB and read them back via every Tier 1 query.

### Architectural notes for this chunk (read before implementing)

**Cypher string escaping.** This chunk establishes a pattern: all Cypher queries are built with `format!()` and inline string literals are escaped via a small `escape()` helper that handles `\\` and `'`. The pattern is deliberate (the spec says "raw Cypher strings via the falkordb crate's query interface") — it's *not* parameterized queries even though the falkordb crate supports them. This is the pattern Chunks 3-8 will copy. **Constraints that follow from this choice:**

- The `escape()` helper handles `\\` and `'` only. Cypher also treats backticks as identifier delimiters; symbol qualified names containing backticks would break queries.
- Therefore the `QualifiedName` constructor in `mycel-core` must reject any input containing a backtick. This is added as a guard in Task 2.1 below — better to reject at ingest than to discover it at query time.
- When future tasks add new query methods, they MUST use `escape()` on every user-supplied string interpolated into Cypher. No exceptions.

**Migration tracking is per-graph.** FalkorDB indices are per-graph, not per-instance. The migration runner records `MigrationRecord` nodes in the meta graph (`mycel:meta`), but each record is scoped by `graph_name`. When the graph layer connects to a new per-repo graph, it must run all migrations again on that graph. The implementation below handles this; if you change the migration runner, preserve that property.

### Task 2.1: `mycel-core` domain types

**Files:**
- Modify: `crates/mycel-core/src/lib.rs`
- Create: `crates/mycel-core/src/symbol.rs`
- Create: `crates/mycel-core/src/edge.rs`
- Create: `crates/mycel-core/src/file.rs`
- Create: `crates/mycel-core/src/manifest.rs`
- Create: `crates/mycel-core/src/error.rs`
- Create: `crates/mycel-core/src/config.rs`

- [ ] **Step 1: Write the failing test for `Symbol` round-trip**

Create `crates/mycel-core/tests/serde.rs`:

```rust
use mycel_core::*;

#[test]
fn symbol_round_trip_json() {
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::foo::Bar"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 10,
        end_line: 20,
        signature: Signature::new("fn bar() -> u32"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
    };
    let json = serde_json::to_string(&sym).unwrap();
    let back: Symbol = serde_json::from_str(&json).unwrap();
    assert_eq!(sym, back);
}

#[test]
fn edge_kind_serializes_lowercase() {
    let e = Edge {
        from: "a".into(),
        to: "b".into(),
        kind: EdgeKind::Calls,
        source: EdgeSource::Lsp,
    };
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("\"calls\""));
    assert!(json.contains("\"lsp\""));
}
```

- [ ] **Step 2: Run test to confirm it fails**

Run: `cargo test -p mycel-core`
Expected: FAIL — types don't exist yet.

- [ ] **Step 3: Implement `src/symbol.rs`**

```rust
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QualifiedName(String);

impl QualifiedName {
    /// Constructs a QualifiedName, replacing any backtick with `_` to avoid
    /// breaking Cypher identifier-delimited string literals downstream.
    /// (Symbols with backticks in their name are vanishingly rare in practice.)
    pub fn new(s: impl Into<String>) -> Self {
        let s = s.into();
        if s.contains('`') {
            Self(s.replace('`', "_"))
        } else {
            Self(s)
        }
    }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Signature(pub String);

impl Signature {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    Type,
    Component,
    Hook,
    Constant,
    Module,
    Struct,
    Enum,
    Trait,
    Impl,
    Static,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    pub qualified_name: QualifiedName,
    pub kind: SymbolKind,
    pub file_path: Utf8PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: Signature,
    pub jsdoc: Option<String>,
    pub synthesized_description: Option<String>,
    pub exported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}
```

- [ ] **Step 4: Implement `src/edge.rs`**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Defines,
    Calls,
    Imports,
    Extends,
    Implements,
    UsesType,
    References,
    ReExports,
    CoChanged,
    TestedBy,
    ModifiedIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeSource {
    TreeSitter,
    Lsp,
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub source: EdgeSource,
}
```

- [ ] **Step 5: Implement `src/file.rs`**

```rust
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: Utf8PathBuf,
    pub language: String,
    pub last_modified: time::OffsetDateTime,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RepoId(pub String);

impl RepoId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}
```

- [ ] **Step 6: Implement `src/manifest.rs`**

```rust
use serde::{Deserialize, Serialize};
use crate::file::RepoId;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexManifest {
    pub repo_id: RepoId,
    pub embedder_identity: String,   // e.g. "ollama/embeddinggemma"
    pub embedder_dimension: u32,     // e.g. 768
    pub schema_version: u32,
    pub last_indexed_at: time::OffsetDateTime,
}
```

- [ ] **Step 7: Implement `src/error.rs`**

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MycelError {
    #[error("graph error: {0}")]
    Graph(String),
    #[error("extraction error in {file}: {message}")]
    Extract { file: String, message: String },
    #[error("LSP error: {0}")]
    Lsp(String),
    #[error("model provider error ({provider}): {message}")]
    Model { provider: String, message: String },
    #[error("config error: {0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("embedder mismatch: graph manifest has {graph}, runtime has {runtime}")]
    EmbedderMismatch { graph: String, runtime: String },
}

pub type Result<T> = std::result::Result<T, MycelError>;
```

- [ ] **Step 8: Implement `src/config.rs`**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Minimal,
    Balanced,
    Max,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsConfig {
    pub tier: Option<Tier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub falkordb_url: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self { falkordb_url: "redis://localhost:6379".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProviderConfig {
    Ollama {
        endpoint: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        dimension: Option<u32>,
    },
}

impl ProviderConfig {
    pub fn default_ollama() -> Self {
        ProviderConfig::Ollama {
            endpoint: "http://localhost:11434".into(),
            model: None,
            dimension: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvidersConfig {
    pub embedder: Option<ProviderConfig>,
    pub synthesizer: Option<ProviderConfig>,
    pub reranker: Option<ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspConfig {
    pub multilspy_path: String,
    pub languages: Vec<String>,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            multilspy_path: "python3 scripts/multilspy_bridge.py".into(),
            languages: vec!["typescript".into(), "rust".into()],
        }
    }
}
```

- [ ] **Step 9: Wire it all up in `src/lib.rs`**

```rust
//! Mycelium core types — pure data, no IO.

pub mod symbol;
pub mod edge;
pub mod file;
pub mod manifest;
pub mod error;
pub mod config;

pub use symbol::*;
pub use edge::*;
pub use file::*;
pub use manifest::*;
pub use error::*;
pub use config::*;
```

- [ ] **Step 10: Run tests, confirm pass**

Run: `cargo test -p mycel-core`
Expected: PASS — both round-trip tests succeed.

- [ ] **Step 11: Commit**

```bash
git add crates/mycel-core
git commit -m "Implement mycel-core domain types with serde round-trips"
```

---

### Task 2.2: `mycel-graph` connection + schema migrations

**Files:**
- Modify: `crates/mycel-graph/src/lib.rs`
- Create: `crates/mycel-graph/src/client.rs`
- Create: `crates/mycel-graph/src/migrations.rs`
- Create: `crates/mycel-graph/tests/integration.rs`

- [ ] **Step 1: Write the failing integration test for `connect` + migration runner**

Create `crates/mycel-graph/tests/integration.rs`:

```rust
//! These tests require a running FalkorDB at $MYCEL_TEST_FALKORDB_URL
//! (defaults to redis://localhost:6379). Run `just up` first.

use mycel_graph::*;

fn url() -> String {
    std::env::var("MYCEL_TEST_FALKORDB_URL")
        .unwrap_or_else(|_| "redis://localhost:6379".into())
}

#[tokio::test]
async fn connect_runs_migrations_idempotently() {
    let client = GraphClient::connect(&url(), "mycel:test:connect").await.unwrap();
    // Should be safe to connect twice in a row.
    let client2 = GraphClient::connect(&url(), "mycel:test:connect").await.unwrap();
    drop((client, client2));
}

#[tokio::test]
async fn migrations_record_in_meta_graph() {
    let client = GraphClient::connect(&url(), "mycel:test:meta").await.unwrap();
    let applied = client.applied_migrations().await.unwrap();
    assert!(applied.contains(&"v1_symbol_indices".to_string()));
    assert!(applied.contains(&"v1_vector_index".to_string()));
}
```

- [ ] **Step 2: Run, confirm it fails**

Run: `just up && cargo test -p mycel-graph`
Expected: FAIL — `GraphClient::connect` does not exist.

- [ ] **Step 3: Implement `src/client.rs`**

```rust
use falkordb::{FalkorClientBuilder, FalkorAsyncClient, FalkorConnectionInfo, FalkorValue};
use mycel_core::{MycelError, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

/// A connected handle to a FalkorDB graph for a specific repo.
///
/// All FalkorDB operations issued through `GraphClient` are raw Cypher
/// against the underlying `falkordb` crate's query interface.
#[derive(Clone)]
pub struct GraphClient {
    inner: Arc<Mutex<FalkorAsyncClient>>,
    pub graph_name: String,
}

impl GraphClient {
    /// Connect to FalkorDB and run migrations.
    pub async fn connect(url: &str, graph_name: &str) -> Result<Self> {
        let info: FalkorConnectionInfo = url
            .try_into()
            .map_err(|e| MycelError::Graph(format!("invalid url {url}: {e:?}")))?;
        let client = FalkorClientBuilder::new_async()
            .with_connection_info(info)
            .build()
            .await
            .map_err(|e| MycelError::Graph(format!("connect: {e}")))?;
        info!(graph = %graph_name, "connected to FalkorDB");
        let me = Self {
            inner: Arc::new(Mutex::new(client)),
            graph_name: graph_name.into(),
        };
        crate::migrations::run_all(&me).await?;
        Ok(me)
    }

    /// Run a Cypher query against this graph and return raw rows.
    ///
    /// Implementation note: `LazyResultSet` from the falkordb crate is a
    /// synchronous `Iterator`, not an async `Stream`. Do **not** add `.await`
    /// to `.collect()`. Also: the `QueryBuilder` returned by `graph.query(...)`
    /// holds a mutable borrow of `graph` for its lifetime — keep `.execute()`
    /// in the same expression and don't store the builder in a local.
    pub async fn query(&self, cypher: &str) -> Result<Vec<Vec<FalkorValue>>> {
        debug!(cypher = %cypher, "issuing cypher");
        let mut guard = self.inner.lock().await;
        let mut graph = guard.select_graph(&self.graph_name);
        let res = graph.query(cypher).execute().await
            .map_err(|e| MycelError::Graph(format!("{e}")))?;
        let rows: Vec<Vec<FalkorValue>> = res.data.collect();
        Ok(rows)
    }

    /// Run a Cypher query against the meta graph (`mycel:meta`).
    pub async fn meta_query(&self, cypher: &str) -> Result<Vec<Vec<FalkorValue>>> {
        debug!(cypher = %cypher, target = "mycel:meta", "issuing meta cypher");
        let mut guard = self.inner.lock().await;
        let mut graph = guard.select_graph("mycel:meta");
        let res = graph.query(cypher).execute().await
            .map_err(|e| MycelError::Graph(format!("{e}")))?;
        let rows: Vec<Vec<FalkorValue>> = res.data.collect();
        Ok(rows)
    }

    /// Returns the IDs of migrations already applied to *this* per-repo graph
    /// (records live in the meta graph but are scoped by the graph name).
    pub async fn applied_migrations(&self) -> Result<Vec<String>> {
        let cypher = format!(
            "MATCH (m:MigrationRecord {{graph_name: '{}'}}) RETURN m.migration_id",
            self.graph_name.replace('\'', "\\'")
        );
        let rows = self.meta_query(&cypher).await?;
        let mut ids = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(FalkorValue::String(s)) = row.into_iter().next() {
                ids.push(s);
            }
        }
        Ok(ids)
    }
}
```

Implementation note: the exact API on `FalkorAsyncClient` / `select_graph` / `query` may differ slightly across crate versions — if `cargo build` fails, consult `cargo doc -p falkordb --open` or `https://docs.rs/falkordb` and adapt. The structure (lock → select graph → query → collect) is what matters.

- [ ] **Step 4: Implement `src/migrations.rs`**

```rust
use crate::GraphClient;
use mycel_core::Result;
use tracing::info;

const MIGRATIONS: &[(&str, &[&str])] = &[
    ("v1_symbol_indices", &[
        "CREATE INDEX FOR (s:Symbol) ON (s.qualified_name)",
        "CREATE INDEX FOR (s:Symbol) ON (s.file_path)",
        "CREATE INDEX FOR (f:File) ON (f.path)",
        "CREATE INDEX FOR (c:Commit) ON (c.sha)",
    ]),
    ("v1_vector_index", &[
        // FalkorDB vector index. Dimension matches EmbeddingGemma 768d.
        "CREATE VECTOR INDEX FOR (s:Symbol) ON (s.embedding) OPTIONS {dimension: 768, similarityFunction: 'cosine'}",
    ]),
];

pub async fn run_all(client: &GraphClient) -> Result<()> {
    // FalkorDB indices are per-graph, so migration tracking must also be per-graph.
    // We store records in the meta graph for centralized inspection, but each record
    // is scoped to a specific graph_name to ensure each repo's graph gets its own
    // index creation pass.
    let applied = client.applied_migrations().await?;
    for (id, statements) in MIGRATIONS {
        if applied.iter().any(|a| a == id) {
            continue;
        }
        info!(graph = %client.graph_name, migration = id, "applying migration");
        for stmt in *statements {
            // CREATE INDEX is idempotent at the FalkorDB layer (errors if exists);
            // we swallow such errors. The per-graph MigrationRecord guard prevents
            // reapplication on subsequent runs.
            let _ = client.query(stmt).await;
        }
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let mark = format!(
            "CREATE (:MigrationRecord {{graph_name: '{g}', migration_id: '{id}', applied_at: {ts}}})",
            g = client.graph_name.replace('\'', "\\'"),
            id = id,
            ts = now
        );
        client.meta_query(&mark).await?;
    }
    Ok(())
}
```

- [ ] **Step 5: Wire `src/lib.rs`**

```rust
//! mycel-graph — sole owner of FalkorDB Cypher.

pub mod client;
pub mod migrations;

pub use client::GraphClient;
```

- [ ] **Step 6: Run tests**

Run: `just up && cargo test -p mycel-graph`
Expected: PASS — connect succeeds; applied_migrations returns the two seeded IDs.

- [ ] **Step 7: Commit**

```bash
git add crates/mycel-graph
git commit -m "Wire GraphClient connect with idempotent migration runner"
```

---

### Task 2.3: `mycel-graph` symbol + edge upserts

**Files:**
- Create: `crates/mycel-graph/src/symbol.rs`
- Create: `crates/mycel-graph/src/edge.rs`
- Modify: `crates/mycel-graph/src/lib.rs`
- Modify: `crates/mycel-graph/tests/integration.rs`

- [ ] **Step 1: Write the failing test for symbol upsert + edge upsert + read-back**

Append to `tests/integration.rs`:

```rust
use mycel_core::*;

#[tokio::test]
async fn symbol_upsert_and_read() {
    let client = GraphClient::connect(&url(), "mycel:test:upsert").await.unwrap();
    let sym = Symbol {
        qualified_name: QualifiedName::new("crate::foo::bar"),
        kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(),
        start_line: 1,
        end_line: 10,
        signature: Signature::new("fn bar() -> u32"),
        jsdoc: None,
        synthesized_description: None,
        exported: true,
        embedding: None,
    };
    client.upsert_symbol(&sym).await.unwrap();
    // Round-trip via a Tier 1-style read
    let definers = client.query_definers("bar").await.unwrap();
    assert_eq!(definers.len(), 1);
    assert_eq!(definers[0].qualified_name.as_str(), "crate::foo::bar");
}

#[tokio::test]
async fn edge_upsert_callers_query() {
    let client = GraphClient::connect(&url(), "mycel:test:edges").await.unwrap();
    let foo = Symbol { qualified_name: QualifiedName::new("crate::foo"), kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(), start_line: 1, end_line: 5,
        signature: Signature::new("fn foo()"), jsdoc: None, synthesized_description: None, exported: true, embedding: None };
    let bar = Symbol { qualified_name: QualifiedName::new("crate::bar"), kind: SymbolKind::Function,
        file_path: "src/lib.rs".into(), start_line: 7, end_line: 12,
        signature: Signature::new("fn bar()"), jsdoc: None, synthesized_description: None, exported: true, embedding: None };
    client.upsert_symbol(&foo).await.unwrap();
    client.upsert_symbol(&bar).await.unwrap();
    client.upsert_edge_batch(&[Edge {
        from: "crate::foo".into(),
        to: "crate::bar".into(),
        kind: EdgeKind::Calls,
        source: EdgeSource::Lsp,
    }]).await.unwrap();
    let callers = client.query_callers("crate::bar").await.unwrap();
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].qualified_name.as_str(), "crate::foo");
}
```

- [ ] **Step 2: Run, confirm fail**

Run: `cargo test -p mycel-graph`
Expected: FAIL — methods don't exist.

- [ ] **Step 3a: Create `src/symbol.rs` with helpers (`escape`, `short_name`, `parse_symbol_row`)**

```rust
use falkordb::FalkorValue;
use mycel_core::*;

pub(crate) fn short_name(qualified: &str) -> &str {
    qualified.rsplit_once("::").map(|(_, n)| n)
        .or_else(|| qualified.rsplit_once('.').map(|(_, n)| n))
        .unwrap_or(qualified)
}

/// Escapes a string for use as a Cypher single-quoted literal.
/// Handles `\\` and `'`. Backticks are forbidden in QualifiedName by construction.
pub(crate) fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Parses a Symbol from a FalkorDB result row matching the order:
/// (qualified_name, kind, file_path, start_line, end_line, signature, exported).
pub(crate) fn parse_symbol_row(row: Vec<FalkorValue>) -> Option<Symbol> {
    let mut iter = row.into_iter();
    let qname = match iter.next()? { FalkorValue::String(s) => s, _ => return None };
    let kind_s = match iter.next()? { FalkorValue::String(s) => s, _ => return None };
    let kind: SymbolKind = serde_json::from_value(serde_json::Value::String(kind_s)).ok()?;
    let file = match iter.next()? { FalkorValue::String(s) => s, _ => return None };
    let start = match iter.next()? { FalkorValue::I64(n) => n as u32, _ => return None };
    let end = match iter.next()? { FalkorValue::I64(n) => n as u32, _ => return None };
    let sig = match iter.next()? { FalkorValue::String(s) => s, _ => return None };
    let exported = match iter.next()? { FalkorValue::Bool(b) => b, _ => return None };
    Some(Symbol {
        qualified_name: QualifiedName::new(qname),
        kind,
        file_path: file.into(),
        start_line: start,
        end_line: end,
        signature: Signature::new(sig),
        jsdoc: None,
        synthesized_description: None,
        exported,
        embedding: None,
    })
}
```

(If `FalkorValue` variants differ in your installed crate version, adjust the match arms accordingly. `cargo doc -p falkordb --open` is the source of truth.)

- [ ] **Step 3b: Append `upsert_symbol` and `upsert_symbol_batch` to `src/symbol.rs`**

```rust
use crate::GraphClient;

impl GraphClient {
    pub async fn upsert_symbol(&self, sym: &Symbol) -> Result<()> {
        let kind = serde_json::to_value(&sym.kind).unwrap();
        let kind_str = kind.as_str().unwrap();
        let cypher = format!(
            r#"MERGE (s:Symbol {{qualified_name: '{qname}'}})
            SET s.kind = '{kind}',
                s.file_path = '{file}',
                s.start_line = {start},
                s.end_line = {end},
                s.signature = '{sig}',
                s.exported = {exported},
                s.name = '{name}'"#,
            qname = escape(sym.qualified_name.as_str()),
            kind = kind_str,
            file = escape(sym.file_path.as_str()),
            start = sym.start_line,
            end = sym.end_line,
            sig = escape(sym.signature.as_str()),
            exported = sym.exported,
            name = escape(short_name(sym.qualified_name.as_str())),
        );
        self.query(&cypher).await?;
        Ok(())
    }

    pub async fn upsert_symbol_batch(&self, syms: &[Symbol]) -> Result<()> {
        for s in syms {
            self.upsert_symbol(s).await?;
        }
        Ok(())
    }
}
```

- [ ] **Step 3c: Append `query_definers` to `src/symbol.rs`**

```rust
impl GraphClient {
    pub async fn query_definers(&self, name: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (s:Symbol) WHERE s.name = '{name}' OR s.qualified_name = '{name}' \
             RETURN s.qualified_name, s.kind, s.file_path, s.start_line, s.end_line, \
                    s.signature, s.exported",
            name = escape(name),
        );
        let rows = self.query(&cypher).await?;
        Ok(rows.into_iter().filter_map(parse_symbol_row).collect())
    }
}
```

- [ ] **Step 4: Implement `src/edge.rs`**

(File: `crates/mycel-graph/src/edge.rs`)

```rust
use crate::{GraphClient, symbol};
use mycel_core::*;

fn edge_kind_to_cypher(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Defines    => "DEFINES",
        EdgeKind::Calls      => "CALLS",
        EdgeKind::Imports    => "IMPORTS",
        EdgeKind::Extends    => "EXTENDS",
        EdgeKind::Implements => "IMPLEMENTS",
        EdgeKind::UsesType   => "USES_TYPE",
        EdgeKind::References => "REFERENCES",
        EdgeKind::ReExports  => "RE_EXPORTS",
        EdgeKind::CoChanged  => "CO_CHANGED",
        EdgeKind::TestedBy   => "TESTED_BY",
        EdgeKind::ModifiedIn => "MODIFIED_IN",
    }
}

impl GraphClient {
    pub async fn upsert_edge_batch(&self, edges: &[Edge]) -> Result<()> {
        for e in edges {
            let cypher = format!(
                r#"MATCH (a:Symbol {{qualified_name: '{from}'}}), (b:Symbol {{qualified_name: '{to}'}})
                   MERGE (a)-[r:{rel}]->(b)
                   SET r.source = '{src}'"#,
                from = symbol::escape(&e.from),
                to = symbol::escape(&e.to),
                rel = edge_kind_to_cypher(e.kind),
                src = serde_json::to_value(&e.source).unwrap().as_str().unwrap(),
            );
            self.query(&cypher).await?;
        }
        Ok(())
    }

    pub async fn query_callers(&self, qualified: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol)-[:CALLS]->(b:Symbol {{qualified_name: '{q}'}}) \
             RETURN a.qualified_name, a.kind, a.file_path, a.start_line, a.end_line, \
                    a.signature, a.exported",
            q = symbol::escape(qualified),
        );
        Ok(self.query(&cypher).await?
            .into_iter()
            .filter_map(symbol::parse_symbol_row)
            .collect())
    }

    pub async fn query_callees(&self, qualified: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol {{qualified_name: '{q}'}})-[:CALLS]->(b:Symbol) \
             RETURN b.qualified_name, b.kind, b.file_path, b.start_line, b.end_line, \
                    b.signature, b.exported",
            q = symbol::escape(qualified),
        );
        Ok(self.query(&cypher).await?
            .into_iter()
            .filter_map(symbol::parse_symbol_row)
            .collect())
    }
}
```

- [ ] **Step 5: Wire `src/lib.rs`**

```rust
//! mycel-graph — sole owner of FalkorDB Cypher.

pub mod client;
pub mod edge;
pub mod migrations;
pub mod symbol;

pub use client::GraphClient;
```

- [ ] **Step 6: Run tests, confirm pass**

Run: `cargo test -p mycel-graph`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/mycel-graph
git commit -m "Add Symbol/Edge upsert and Tier 1 callers/callees/definers queries"
```

---

### Task 2.4: Remaining Tier 1 queries + manifest read/write + vector search

**Files:**
- Create: `crates/mycel-graph/src/queries.rs`
- Create: `crates/mycel-graph/src/manifest.rs`
- Create: `crates/mycel-graph/src/vector.rs`
- Modify: `crates/mycel-graph/src/lib.rs`
- Modify: `crates/mycel-graph/tests/integration.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/integration.rs`:

```rust
#[tokio::test]
async fn imports_uses_implements_queries() {
    let client = GraphClient::connect(&url(), "mycel:test:tier1").await.unwrap();
    // Set up a tiny graph: file imports symbol; symbol uses type; class implements interface
    let foo = Symbol { qualified_name: QualifiedName::new("ts::Foo"), kind: SymbolKind::Class,
        file_path: "src/foo.ts".into(), start_line: 1, end_line: 3,
        signature: Signature::new("class Foo"), jsdoc: None, synthesized_description: None, exported: true, embedding: None };
    let i = Symbol { qualified_name: QualifiedName::new("ts::IFoo"), kind: SymbolKind::Interface,
        file_path: "src/foo.ts".into(), start_line: 5, end_line: 6,
        signature: Signature::new("interface IFoo"), jsdoc: None, synthesized_description: None, exported: true, embedding: None };
    client.upsert_symbol(&foo).await.unwrap();
    client.upsert_symbol(&i).await.unwrap();
    client.upsert_edge_batch(&[
        Edge { from: "ts::Foo".into(), to: "ts::IFoo".into(), kind: EdgeKind::Implements, source: EdgeSource::Lsp },
    ]).await.unwrap();
    let impls = client.query_implements("ts::IFoo").await.unwrap();
    assert!(impls.iter().any(|s| s.qualified_name.as_str() == "ts::Foo"));
}

#[tokio::test]
async fn manifest_round_trip() {
    use mycel_core::*;
    let client = GraphClient::connect(&url(), "mycel:test:manifest").await.unwrap();
    let m = IndexManifest {
        repo_id: RepoId::new("test-repo"),
        embedder_identity: "ollama/embeddinggemma".into(),
        embedder_dimension: 768,
        schema_version: SCHEMA_VERSION,
        last_indexed_at: time::OffsetDateTime::now_utc(),
    };
    client.write_manifest(&m).await.unwrap();
    let read = client.read_manifest("test-repo").await.unwrap().unwrap();
    assert_eq!(read.embedder_identity, m.embedder_identity);
    assert_eq!(read.embedder_dimension, 768);
}
```

- [ ] **Step 2: Run, confirm fail**

Run: `cargo test -p mycel-graph`
Expected: FAIL — `query_implements` and `read_manifest` not yet defined.

- [ ] **Step 3: Implement `src/queries.rs`**

```rust
use crate::{GraphClient, symbol};
use mycel_core::*;

impl GraphClient {
    /// "Who imports from this file?"
    ///
    /// Per DESIGN.md schema, `IMPORTS` edges can be either `Symbol→Symbol`
    /// (e.g., a TS function imports another exported function) or `File→File`
    /// (one file imports another). Phase 1 covers the Symbol→Symbol case,
    /// which is the dominant pattern in TS/Rust. The File→File case is added
    /// in Chunk 3 once the extractor produces those edges; revisit this query
    /// then to UNION in the file-level edges.
    pub async fn query_imports(&self, file_path: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol)-[:IMPORTS]->(b:Symbol) WHERE b.file_path = '{p}' \
             RETURN DISTINCT a.qualified_name, a.kind, a.file_path, a.start_line, a.end_line, \
                    a.signature, a.exported",
            p = symbol::escape(file_path),
        );
        Ok(self.query(&cypher).await?
            .into_iter()
            .filter_map(symbol::parse_symbol_row)
            .collect())
    }

    pub async fn query_uses(&self, type_qname: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol)-[:USES_TYPE]->(t:Symbol {{qualified_name: '{q}'}}) \
             RETURN DISTINCT a.qualified_name, a.kind, a.file_path, a.start_line, a.end_line, \
                    a.signature, a.exported",
            q = symbol::escape(type_qname),
        );
        Ok(self.query(&cypher).await?
            .into_iter()
            .filter_map(symbol::parse_symbol_row)
            .collect())
    }

    pub async fn query_implements(&self, interface_qname: &str) -> Result<Vec<Symbol>> {
        let cypher = format!(
            "MATCH (a:Symbol)-[:IMPLEMENTS]->(b:Symbol {{qualified_name: '{q}'}}) \
             RETURN a.qualified_name, a.kind, a.file_path, a.start_line, a.end_line, \
                    a.signature, a.exported",
            q = symbol::escape(interface_qname),
        );
        Ok(self.query(&cypher).await?
            .into_iter()
            .filter_map(symbol::parse_symbol_row)
            .collect())
    }
}
```

- [ ] **Step 4: Implement `src/manifest.rs`**

```rust
use crate::GraphClient;
use falkordb::FalkorValue;
use mycel_core::*;

impl GraphClient {
    pub async fn write_manifest(&self, m: &IndexManifest) -> Result<()> {
        let cypher = format!(
            r#"MERGE (i:IndexManifest {{repo_id: '{repo}'}})
               SET i.embedder_identity = '{embed_id}',
                   i.embedder_dimension = {dim},
                   i.schema_version = {schema},
                   i.last_indexed_at = {ts}"#,
            repo = crate::symbol::escape(m.repo_id.as_str()),
            embed_id = crate::symbol::escape(&m.embedder_identity),
            dim = m.embedder_dimension,
            schema = m.schema_version,
            ts = m.last_indexed_at.unix_timestamp(),
        );
        self.meta_query(&cypher).await?;
        Ok(())
    }

    pub async fn read_manifest(&self, repo_id: &str) -> Result<Option<IndexManifest>> {
        let cypher = format!(
            "MATCH (i:IndexManifest {{repo_id: '{repo}'}}) \
             RETURN i.repo_id, i.embedder_identity, i.embedder_dimension, i.schema_version, i.last_indexed_at",
            repo = crate::symbol::escape(repo_id),
        );
        let rows = self.meta_query(&cypher).await?;
        let Some(row) = rows.into_iter().next() else { return Ok(None); };
        let mut iter = row.into_iter();
        let repo = match iter.next() { Some(FalkorValue::String(s)) => s, _ => return Ok(None) };
        let embed_id = match iter.next() { Some(FalkorValue::String(s)) => s, _ => return Ok(None) };
        let dim = match iter.next() { Some(FalkorValue::I64(n)) => n as u32, _ => return Ok(None) };
        let schema = match iter.next() { Some(FalkorValue::I64(n)) => n as u32, _ => return Ok(None) };
        let ts = match iter.next() { Some(FalkorValue::I64(n)) => n, _ => return Ok(None) };
        Ok(Some(IndexManifest {
            repo_id: RepoId::new(repo),
            embedder_identity: embed_id,
            embedder_dimension: dim,
            schema_version: schema,
            last_indexed_at: time::OffsetDateTime::from_unix_timestamp(ts)
                .map_err(|e| MycelError::Graph(e.to_string()))?,
        }))
    }
}
```

- [ ] **Step 5: Implement `src/vector.rs`**

```rust
use crate::GraphClient;
use falkordb::FalkorValue;
use mycel_core::*;

impl GraphClient {
    /// Vector search the top-K symbols by cosine similarity to the given query vector.
    /// Returns (Symbol, score) pairs.
    pub async fn vector_search_top_k(
        &self,
        query: &[f32],
        k: usize,
        expected_embedder: &str,
        expected_dim: u32,
    ) -> Result<Vec<(Symbol, f32)>> {
        // Embedder safety check: the per-repo manifest must agree with the runtime config.
        if let Some(m) = self.read_manifest(&self.graph_name).await? {
            if m.embedder_identity != expected_embedder || m.embedder_dimension != expected_dim {
                return Err(MycelError::EmbedderMismatch {
                    graph: format!("{}@{}", m.embedder_identity, m.embedder_dimension),
                    runtime: format!("{}@{}", expected_embedder, expected_dim),
                });
            }
        }
        let vec_lit = format!("vecf32([{}])", query.iter()
            .map(|f| format!("{f}"))
            .collect::<Vec<_>>().join(","));
        let cypher = format!(
            "CALL db.idx.vector.queryNodes('Symbol', 'embedding', {k}, {vec}) \
             YIELD node, score \
             RETURN node.qualified_name, node.kind, node.file_path, node.start_line, node.end_line, \
                    node.signature, node.exported, score",
            vec = vec_lit,
        );
        let rows = self.query(&cypher).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut iter = row.into_iter();
            // Parse symbol fields, then score
            let symbol = crate::symbol::parse_symbol_row(iter.by_ref().take(7).collect());
            if let Some(s) = symbol {
                let score = match iter.next() {
                    Some(FalkorValue::F64(f)) => f as f32,
                    _ => 0.0,
                };
                out.push((s, score));
            }
        }
        Ok(out)
    }
}
```

- [ ] **Step 6: Update `src/lib.rs`**

```rust
//! mycel-graph — sole owner of FalkorDB Cypher.

pub mod client;
pub mod edge;
pub mod manifest;
pub mod migrations;
pub mod queries;
pub mod symbol;
pub mod vector;

pub use client::GraphClient;
```

- [ ] **Step 7: Run tests, confirm pass**

Run: `cargo test -p mycel-graph`
Expected: all integration tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/mycel-graph
git commit -m "Add remaining Tier 1 queries, manifest persistence, vector search"
```

---

## Chunk 3: Extraction (tree-sitter for TS + Rust)

This chunk produces `mycel-extract` with both `TypeScriptExtractor` and `RustExtractor`. Each extractor takes a parsed file and emits an `ExtractionOutput` (symbols + tentative tree-sitter-source edges). All edges produced by this layer carry `EdgeSource::TreeSitter`. Golden snapshot tests fix the output for representative fixtures so future regressions fail loudly.

### Architectural notes

- The two extractors share a common shape but tree-sitter queries are language-specific. Don't try to write one extractor that handles both — duplicate cleanly.
- `ExtractionOutput` is pure data. It does NOT touch FalkorDB.
- Use `tree-sitter` queries (the `.scm` query DSL via `Query::new`) rather than walking the AST manually — queries are concise and easier to maintain.
- Snapshot tests use `cargo insta`. To approve new snapshots: `cargo insta review`.

### Task 3.1: Define `Extractor` trait + `ExtractionOutput` + factory

**Files:**
- Modify: `crates/mycel-extract/src/lib.rs`

- [ ] **Step 1: Write the trait + types**

```rust
//! Per-language tree-sitter extractors. All edges produced here carry
//! EdgeSource::TreeSitter; LSP refinement (mycel-lsp) upgrades them later.

use camino::Utf8Path;
use mycel_core::*;

pub mod languages;

#[derive(Debug, Clone, Default)]
pub struct ExtractionOutput {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
    pub language: String,
}

pub trait Extractor: Send + Sync {
    fn language_name(&self) -> &'static str;
    fn extract(&self, file: &Utf8Path, content: &str) -> Result<ExtractionOutput>;
}

/// Returns the right extractor for a given file extension, or None if
/// no Phase 1 extractor handles it.
pub fn for_language(file: &Utf8Path) -> Option<Box<dyn Extractor>> {
    match file.extension()? {
        "ts" | "tsx" | "js" | "jsx" | "mts" | "cts" =>
            Some(Box::new(languages::typescript::TypeScriptExtractor::new())),
        "rs" =>
            Some(Box::new(languages::rust::RustExtractor::new())),
        _ => None,
    }
}
```

- [ ] **Step 2: Create `src/languages/mod.rs`**

```rust
pub mod typescript;
pub mod rust;
```

- [ ] **Step 3: Confirm build (extractors are stubs, won't be referenced yet)**

Run: `cargo build -p mycel-extract`
Expected: compile errors because `typescript` and `rust` modules don't exist yet. Move on to next task.

### Task 3.2: TypeScript extractor

**Files:**
- Create: `crates/mycel-extract/src/languages/typescript.rs`
- Create: `tests/fixtures/typescript/simple_function.ts`
- Create: `tests/fixtures/typescript/class_with_methods.ts`
- Create: `tests/fixtures/typescript/imports_and_exports.ts`
- Create: `crates/mycel-extract/tests/typescript_fixtures.rs`

- [ ] **Step 1: Create three minimal fixture files**

`tests/fixtures/typescript/simple_function.ts`:
```typescript
export function add(a: number, b: number): number {
  return a + b;
}

function double(x: number): number {
  return add(x, x);
}
```

`tests/fixtures/typescript/class_with_methods.ts`:
```typescript
export interface Greeter {
  greet(name: string): string;
}

export class FormalGreeter implements Greeter {
  prefix: string = "Hello, ";
  greet(name: string): string {
    return this.prefix + name;
  }
}
```

`tests/fixtures/typescript/imports_and_exports.ts`:
```typescript
import { add } from "./simple_function";
import type { Greeter } from "./class_with_methods";

export const greet: Greeter = {
  greet(name: string): string {
    return `Hi ${name}, sum is ${add(1, 2)}`;
  },
};
```

- [ ] **Step 2: Write the failing snapshot test**

Create `crates/mycel-extract/tests/typescript_fixtures.rs`:

```rust
use camino::Utf8PathBuf;
use mycel_extract::*;

fn extract_fixture(name: &str) -> ExtractionOutput {
    let path: Utf8PathBuf = format!("../../tests/fixtures/typescript/{name}").into();
    let content = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let extractor = for_language(&path).expect("ts extractor");
    extractor.extract(&path, &content).unwrap()
}

#[test]
fn simple_function_snapshot() {
    insta::assert_yaml_snapshot!(extract_fixture("simple_function.ts"));
}

#[test]
fn class_with_methods_snapshot() {
    insta::assert_yaml_snapshot!(extract_fixture("class_with_methods.ts"));
}

#[test]
fn imports_and_exports_snapshot() {
    insta::assert_yaml_snapshot!(extract_fixture("imports_and_exports.ts"));
}
```

Add `insta` to `mycel-extract`'s `[dev-dependencies]` in `Cargo.toml`:
```toml
[dev-dependencies]
insta = { workspace = true }
```

- [ ] **Step 3: Run; confirm fail (no typescript module)**

Run: `cargo test -p mycel-extract`
Expected: FAIL — module doesn't exist.

- [ ] **Step 4a: Implement `TypeScriptExtractor` skeleton**

```rust
// crates/mycel-extract/src/languages/typescript.rs
use camino::Utf8Path;
use mycel_core::*;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor, Tree, Node};
use crate::{Extractor, ExtractionOutput};

// Note: tree-sitter 0.25 returns a `StreamingIterator` from
// `QueryCursor::matches`; you cannot use `for m in ...`. Use
// `while let Some(m) = matches.next()` after `use streaming_iterator::StreamingIterator;`.
// Add `streaming-iterator = "0.1"` to mycel-extract Cargo.toml.

pub struct TypeScriptExtractor {
    ts_lang: Language,
    tsx_lang: Language,
}

impl Default for TypeScriptExtractor { fn default() -> Self { Self::new() } }

impl TypeScriptExtractor {
    pub fn new() -> Self {
        Self {
            ts_lang: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tsx_lang: tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    fn language_for(&self, path: &Utf8Path) -> Language {
        match path.extension() {
            Some("tsx") | Some("jsx") => self.tsx_lang.clone(),
            _ => self.ts_lang.clone(),
        }
    }
}

impl Extractor for TypeScriptExtractor {
    fn language_name(&self) -> &'static str { "typescript" }

    fn extract(&self, file: &Utf8Path, content: &str) -> Result<ExtractionOutput> {
        let mut parser = Parser::new();
        let lang = self.language_for(file);
        parser.set_language(&lang)
            .map_err(|e| MycelError::Extract { file: file.to_string(), message: format!("set_language: {e}") })?;
        let tree: Tree = parser.parse(content, None)
            .ok_or_else(|| MycelError::Extract { file: file.to_string(), message: "parse failed".into() })?;

        let mut symbols = Vec::new();
        let mut edges = Vec::new();
        extract_symbols_and_edges(&lang, &tree, content, file, &mut symbols, &mut edges)?;

        Ok(ExtractionOutput { symbols, edges, language: "typescript".into() })
    }
}

fn extract_symbols_and_edges(
    lang: &Language,
    tree: &Tree,
    src: &str,
    file: &Utf8Path,
    symbols: &mut Vec<Symbol>,
    edges: &mut Vec<Edge>,
) -> Result<()> {
    // Symbols: functions, classes, interfaces, types, methods.
    let q = Query::new(lang, SYMBOLS_QUERY)
        .map_err(|e| MycelError::Extract { file: file.to_string(), message: format!("ts query: {e}") })?;
    let mut cursor = QueryCursor::new();
    let bytes = src.as_bytes();
    let mut matches = cursor.matches(&q, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let mut name: Option<&str> = None;
        let mut kind: Option<SymbolKind> = None;
        let mut start_node: Option<Node> = None;
        let mut end_node: Option<Node> = None;
        let mut signature_node: Option<Node> = None;
        let mut exported = false;
        for c in m.captures {
            let cap = q.capture_names()[c.index as usize];
            match cap.as_ref() {
                "fn.name" => { name = Some(c.node.utf8_text(bytes).unwrap_or("")); kind = Some(SymbolKind::Function); }
                "method.name" => { name = Some(c.node.utf8_text(bytes).unwrap_or("")); kind = Some(SymbolKind::Method); }
                "class.name" => { name = Some(c.node.utf8_text(bytes).unwrap_or("")); kind = Some(SymbolKind::Class); }
                "interface.name" => { name = Some(c.node.utf8_text(bytes).unwrap_or("")); kind = Some(SymbolKind::Interface); }
                "type.name" => { name = Some(c.node.utf8_text(bytes).unwrap_or("")); kind = Some(SymbolKind::Type); }
                "fn.def" | "method.def" | "class.def" | "interface.def" | "type.def" => {
                    start_node = Some(c.node);
                    end_node = Some(c.node);
                    signature_node = Some(c.node);
                }
                "exported" => exported = true,
                _ => {}
            }
        }
        if let (Some(name), Some(kind), Some(start), Some(end)) = (name, kind, start_node, end_node) {
            let sig_text = signature_node
                .and_then(|n| signature_first_line(n, src))
                .unwrap_or_else(|| name.into());
            symbols.push(Symbol {
                qualified_name: QualifiedName::new(format!("{}::{}", file.as_str(), name)),
                kind,
                file_path: file.to_path_buf(),
                start_line: start.start_position().row as u32 + 1,
                end_line: end.end_position().row as u32 + 1,
                signature: Signature::new(sig_text),
                jsdoc: None,
                synthesized_description: None,
                exported,
                embedding: None,
            });
        }
    }

    // Calls: function/method invocations. Resolve the *enclosing* function for each
    // call site by walking parents from the call node — the simplified CALLS_QUERY
    // captures only the call site; we derive `caller` here.
    let q_calls = Query::new(lang, CALLS_QUERY)
        .map_err(|e| MycelError::Extract { file: file.to_string(), message: format!("ts calls: {e}") })?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q_calls, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let mut callee_node: Option<Node> = None;
        let mut site_node: Option<Node> = None;
        for c in m.captures {
            match q_calls.capture_names()[c.index as usize].as_ref() {
                "callee" => callee_node = Some(c.node),
                "site" => site_node = Some(c.node),
                _ => {}
            }
        }
        let (Some(callee), Some(site)) = (callee_node, site_node) else { continue };
        let callee_name = callee.utf8_text(bytes).unwrap_or("");
        if callee_name.is_empty() { continue; }
        let caller = enclosing_named_decl(site, bytes).unwrap_or("<file>");
        edges.push(Edge {
            from: format!("{}::{}", file.as_str(), caller),
            to: callee_name.into(),
            kind: EdgeKind::Calls,
            source: EdgeSource::TreeSitter,
        });
    }

    // Imports: file-level import statements.
    let q_imports = Query::new(lang, IMPORTS_QUERY)
        .map_err(|e| MycelError::Extract { file: file.to_string(), message: format!("ts imports: {e}") })?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q_imports, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        for c in m.captures {
            if q_imports.capture_names()[c.index as usize].as_ref() == "import.source" {
                let src_text = c.node.utf8_text(bytes).unwrap_or("").trim_matches('"').trim_matches('\'');
                edges.push(Edge {
                    from: file.as_str().into(),
                    to: src_text.into(),
                    kind: EdgeKind::Imports,
                    source: EdgeSource::TreeSitter,
                });
            }
        }
    }

    Ok(())
}

/// Walks parents of `node` to find the nearest function/method/class declaration
/// and returns its name (best-effort).
fn enclosing_named_decl<'a>(node: Node, bytes: &'a [u8]) -> Option<&'a str> {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        match parent.kind() {
            "function_declaration" | "function" | "method_definition" | "arrow_function" => {
                if let Some(name_node) = parent.child_by_field_name("name") {
                    return name_node.utf8_text(bytes).ok();
                }
                return Some("<anonymous>");
            }
            "class_declaration" => {
                if let Some(name_node) = parent.child_by_field_name("name") {
                    return name_node.utf8_text(bytes).ok();
                }
            }
            _ => {}
        }
        cur = parent;
    }
    None
}

fn signature_first_line(n: Node, src: &str) -> Option<String> {
    let text = n.utf8_text(src.as_bytes()).ok()?;
    text.lines().next().map(|s| s.trim().to_string())
}

const SYMBOLS_QUERY: &str = r#"
(function_declaration name: (identifier) @fn.name) @fn.def
(method_definition name: (property_identifier) @method.name) @method.def
(class_declaration name: (type_identifier) @class.name) @class.def
(interface_declaration name: (type_identifier) @interface.name) @interface.def
(type_alias_declaration name: (type_identifier) @type.name) @type.def
(export_statement (function_declaration name: (identifier) @fn.name)) @exported
(export_statement (class_declaration name: (type_identifier) @class.name)) @exported
"#;

const CALLS_QUERY: &str = r#"
(call_expression function: (identifier) @callee) @site
(call_expression function: (member_expression property: (property_identifier) @callee)) @site
"#;

const IMPORTS_QUERY: &str = r#"
(import_statement source: (string) @import.source)
"#;
```

Note on tree-sitter API: the exact `tree-sitter` 0.25 API for `Query::new` and `cursor.matches(...)` may differ slightly across releases. If something fails to compile, run `cargo doc -p tree-sitter --open` and adjust call sites. Specifically `LANGUAGE_TYPESCRIPT.into()` may need to be `tree_sitter_typescript::LANGUAGE_TYPESCRIPT` or `tree_sitter_typescript::language_typescript()` depending on version — check the published crate's actual exports.

The `caller_fn`/`caller_method` capture used above isn't expressed in the simplified `CALLS_QUERY`; for Phase 1 a coarse heuristic that finds calls inside the most recent enclosing function is acceptable. The `mycel-lsp` chunk replaces these heuristic edges with precise LSP-resolved ones; the tree-sitter pass only needs to produce *plausible* call edges.

- [ ] **Step 4b: Run the snapshot test, accept initial snapshots**

Run: `cargo test -p mycel-extract`
Expected: snapshot tests fail (no existing snapshots).

Run: `cargo insta review`
Approve each snapshot after eyeballing for sanity (function names present, classes present, imports present).

Re-run: `cargo test -p mycel-extract`
Expected: PASS — snapshots match.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-extract tests/fixtures/typescript
git commit -m "Add TypeScriptExtractor with golden snapshot tests"
```

### Task 3.3: Rust extractor

**Files:**
- Create: `crates/mycel-extract/src/languages/rust.rs`
- Create: `tests/fixtures/rust/simple_module.rs`
- Create: `tests/fixtures/rust/trait_and_impl.rs`
- Create: `crates/mycel-extract/tests/rust_fixtures.rs`

- [ ] **Step 1: Create fixture files**

`tests/fixtures/rust/simple_module.rs`:
```rust
pub fn add(a: u32, b: u32) -> u32 {
    a + b
}

fn double(x: u32) -> u32 {
    add(x, x)
}

pub const ANSWER: u32 = 42;
```

`tests/fixtures/rust/trait_and_impl.rs`:
```rust
pub trait Greeter {
    fn greet(&self, name: &str) -> String;
}

pub struct FormalGreeter {
    prefix: String,
}

impl Greeter for FormalGreeter {
    fn greet(&self, name: &str) -> String {
        format!("{}{}", self.prefix, name)
    }
}
```

- [ ] **Step 2: Write failing snapshot tests**

Create `crates/mycel-extract/tests/rust_fixtures.rs`:

```rust
use camino::Utf8PathBuf;
use mycel_extract::*;

fn extract_fixture(name: &str) -> ExtractionOutput {
    let path: Utf8PathBuf = format!("../../tests/fixtures/rust/{name}").into();
    let content = std::fs::read_to_string(&path).unwrap();
    let extractor = for_language(&path).expect("rust extractor");
    extractor.extract(&path, &content).unwrap()
}

#[test]
fn simple_module_snapshot() {
    insta::assert_yaml_snapshot!(extract_fixture("simple_module.rs"));
}

#[test]
fn trait_and_impl_snapshot() {
    insta::assert_yaml_snapshot!(extract_fixture("trait_and_impl.rs"));
}
```

- [ ] **Step 3: Run; confirm fail**

Run: `cargo test -p mycel-extract`
Expected: FAIL — `RustExtractor` doesn't exist.

- [ ] **Step 4: Implement `RustExtractor`**

```rust
// crates/mycel-extract/src/languages/rust.rs
use camino::Utf8Path;
use mycel_core::*;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor, Tree, Node};
use crate::{Extractor, ExtractionOutput};

pub struct RustExtractor { lang: Language }

impl Default for RustExtractor { fn default() -> Self { Self::new() } }

impl RustExtractor {
    pub fn new() -> Self { Self { lang: tree_sitter_rust::LANGUAGE.into() } }
}

impl Extractor for RustExtractor {
    fn language_name(&self) -> &'static str { "rust" }

    fn extract(&self, file: &Utf8Path, content: &str) -> Result<ExtractionOutput> {
        let mut parser = Parser::new();
        parser.set_language(&self.lang)
            .map_err(|e| MycelError::Extract { file: file.to_string(), message: format!("set_language: {e}") })?;
        let tree: Tree = parser.parse(content, None)
            .ok_or_else(|| MycelError::Extract { file: file.to_string(), message: "parse failed".into() })?;
        let mut symbols = Vec::new();
        let mut edges = Vec::new();
        extract_rust(&self.lang, &tree, content, file, &mut symbols, &mut edges)?;
        Ok(ExtractionOutput { symbols, edges, language: "rust".into() })
    }
}

fn extract_rust(
    lang: &Language, tree: &Tree, src: &str, file: &Utf8Path,
    symbols: &mut Vec<Symbol>, edges: &mut Vec<Edge>,
) -> Result<()> {
    let q = Query::new(lang, RUST_SYMBOLS).map_err(|e| MycelError::Extract { file: file.to_string(), message: format!("rs query: {e}") })?;
    let mut cursor = QueryCursor::new();
    let bytes = src.as_bytes();
    let mut matches = cursor.matches(&q, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let mut name: Option<&str> = None;
        let mut kind: Option<SymbolKind> = None;
        let mut def_node: Option<Node> = None;
        let mut visible = false;
        for c in m.captures {
            match q.capture_names()[c.index as usize].as_ref() {
                "fn.name"      => { name = node_text(c.node, bytes); kind = Some(SymbolKind::Function); }
                "struct.name"  => { name = node_text(c.node, bytes); kind = Some(SymbolKind::Struct); }
                "enum.name"    => { name = node_text(c.node, bytes); kind = Some(SymbolKind::Enum); }
                "trait.name"   => { name = node_text(c.node, bytes); kind = Some(SymbolKind::Trait); }
                "type.name"    => { name = node_text(c.node, bytes); kind = Some(SymbolKind::Type); }
                "const.name"   => { name = node_text(c.node, bytes); kind = Some(SymbolKind::Constant); }
                "static.name"  => { name = node_text(c.node, bytes); kind = Some(SymbolKind::Static); }
                "mod.name"     => { name = node_text(c.node, bytes); kind = Some(SymbolKind::Module); }
                "fn.def" | "struct.def" | "enum.def" | "trait.def" | "type.def"
                | "const.def" | "static.def" | "mod.def" => def_node = Some(c.node),
                "vis.pub" => visible = true,
                _ => {}
            }
        }
        if let (Some(name), Some(kind), Some(def)) = (name, kind, def_node) {
            symbols.push(Symbol {
                qualified_name: QualifiedName::new(format!("{}::{}", file.as_str(), name)),
                kind,
                file_path: file.to_path_buf(),
                start_line: def.start_position().row as u32 + 1,
                end_line: def.end_position().row as u32 + 1,
                signature: Signature::new(
                    def.utf8_text(bytes).ok().and_then(|t| t.lines().next().map(|l| l.trim().to_string()))
                        .unwrap_or_else(|| name.into())
                ),
                jsdoc: None,
                synthesized_description: None,
                exported: visible,
                embedding: None,
            });
        }
    }

    // Impl blocks → IMPLEMENTS edges
    let q_impl = Query::new(lang, RUST_IMPLS).map_err(|e| MycelError::Extract { file: file.to_string(), message: format!("rs impls: {e}") })?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q_impl, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let mut trait_name: Option<&str> = None;
        let mut type_name: Option<&str> = None;
        for c in m.captures {
            match q_impl.capture_names()[c.index as usize].as_ref() {
                "trait" => trait_name = node_text(c.node, bytes),
                "ty"    => type_name  = node_text(c.node, bytes),
                _ => {}
            }
        }
        if let (Some(trait_name), Some(type_name)) = (trait_name, type_name) {
            edges.push(Edge {
                from: format!("{}::{}", file.as_str(), type_name),
                to: trait_name.into(),
                kind: EdgeKind::Implements,
                source: EdgeSource::TreeSitter,
            });
        }
    }

    // use statements → IMPORTS
    let q_use = Query::new(lang, RUST_USES).map_err(|e| MycelError::Extract { file: file.to_string(), message: format!("rs uses: {e}") })?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q_use, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        for c in m.captures {
            if q_use.capture_names()[c.index as usize].as_ref() == "use.path" {
                let path = node_text(c.node, bytes).unwrap_or("");
                edges.push(Edge {
                    from: file.as_str().into(),
                    to: path.into(),
                    kind: EdgeKind::Imports,
                    source: EdgeSource::TreeSitter,
                });
            }
        }
    }

    // Calls — derive enclosing function as caller (heuristic; LSP refines later).
    let q_call = Query::new(lang, RUST_CALLS).map_err(|e| MycelError::Extract { file: file.to_string(), message: format!("rs calls: {e}") })?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q_call, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        for c in m.captures {
            if q_call.capture_names()[c.index as usize].as_ref() == "callee" {
                let callee = node_text(c.node, bytes).unwrap_or("");
                if callee.is_empty() { continue; }
                let caller = enclosing_fn(c.node, bytes).unwrap_or("<file>");
                edges.push(Edge {
                    from: format!("{}::{}", file.as_str(), caller),
                    to: callee.into(),
                    kind: EdgeKind::Calls,
                    source: EdgeSource::TreeSitter,
                });
            }
        }
    }

    Ok(())
}

fn enclosing_fn<'a>(node: Node, bytes: &'a [u8]) -> Option<&'a str> {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        if matches!(parent.kind(), "function_item" | "function_signature_item") {
            if let Some(name) = parent.child_by_field_name("name") {
                return name.utf8_text(bytes).ok();
            }
        }
        cur = parent;
    }
    None
}

fn node_text<'s>(n: Node, src: &'s [u8]) -> Option<&'s str> { n.utf8_text(src).ok() }

// IMPORTANT — these tree-sitter-rust queries must be validated against the
// installed grammar version before claiming the implementation works. See
// Step 4b below for a verification step that calls Query::new(...) on each
// constant and surfaces any "field not found" / "unknown node" errors early.
//
// Field names that may differ across tree-sitter-rust versions:
// - function_item: `name:`  (stable)
// - struct_item:   `name:`  (stable)
// - trait_item:    `name:`  (stable)
// - type_item:     `name:`  — verify; falls back to (identifier) child if missing
// - const_item:    `name:`  — verify
// - static_item:   `name:`  — verify
// - mod_item:      `name:`  — verify
// - impl_item:     `trait:` and `type:` exist; type may be (generic_type), not
//                  just (type_identifier) — query below handles both.
// If `Query::new(...)` returns a TSQueryError on any constant, drop the
// problematic line and replace with a child-walking match in the impl body.

const RUST_SYMBOLS: &str = r#"
(function_item name: (identifier) @fn.name) @fn.def
(struct_item   name: (type_identifier) @struct.name) @struct.def
(enum_item     name: (type_identifier) @enum.name)   @enum.def
(trait_item    name: (type_identifier) @trait.name)  @trait.def
(type_item     name: (type_identifier) @type.name)   @type.def
(const_item    name: (identifier) @const.name)       @const.def
(static_item   name: (identifier) @static.name)      @static.def
(mod_item      name: (identifier) @mod.name)         @mod.def
"#;

// `type:` in an impl_item may be a `type_identifier` (for `impl Trait for Foo`)
// or a `generic_type` / `reference_type` etc. Match both common shapes.
const RUST_IMPLS: &str = r#"
(impl_item trait: (type_identifier) @trait type: (type_identifier) @ty)
(impl_item trait: (type_identifier) @trait type: (generic_type type: (type_identifier) @ty))
"#;

const RUST_USES: &str = r#"
(use_declaration argument: (_) @use.path)
"#;

const RUST_CALLS: &str = r#"
(call_expression function: (identifier) @callee)
(call_expression function: (field_expression field: (field_identifier) @callee))
(call_expression function: (scoped_identifier name: (identifier) @callee))
"#;
```

- [ ] **Step 4b: Verify each tree-sitter query compiles before running fixture tests**

Add `crates/mycel-extract/tests/query_compile.rs`:

```rust
//! Sanity check: every Query::new must succeed on the installed grammars.
//! If any of these tests fails, the offending query string targets a
//! field/node name that doesn't exist in the current tree-sitter grammar
//! version. Fix the query, do not skip the test.

use tree_sitter::{Language, Query};

fn try_compile(name: &str, lang: Language, src: &str) {
    Query::new(&lang, src).unwrap_or_else(|e| panic!("query `{name}` failed to compile: {e}"));
}

#[test]
fn typescript_queries_compile() {
    let ts: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    let tsx: Language = tree_sitter_typescript::LANGUAGE_TSX.into();
    // Use the same constants from the typescript module via private re-export
    // OR re-paste them here for the compile-check test. Easier: re-paste for now.
    let symbols = include_str!("../src/languages/typescript_queries/symbols.scm");
    let calls = include_str!("../src/languages/typescript_queries/calls.scm");
    let imports = include_str!("../src/languages/typescript_queries/imports.scm");
    for (label, src) in &[("symbols", symbols), ("calls", calls), ("imports", imports)] {
        try_compile(&format!("ts/{label}"), ts.clone(), src);
        try_compile(&format!("tsx/{label}"), tsx.clone(), src);
    }
}

#[test]
fn rust_queries_compile() {
    let lang: Language = tree_sitter_rust::LANGUAGE.into();
    let symbols = include_str!("../src/languages/rust_queries/symbols.scm");
    let impls = include_str!("../src/languages/rust_queries/impls.scm");
    let uses = include_str!("../src/languages/rust_queries/uses.scm");
    let calls = include_str!("../src/languages/rust_queries/calls.scm");
    for (label, src) in &[("symbols", symbols), ("impls", impls), ("uses", uses), ("calls", calls)] {
        try_compile(&format!("rs/{label}"), lang.clone(), src);
    }
}
```

This requires extracting the inline query constants into per-language `.scm` files under `crates/mycel-extract/src/languages/typescript_queries/` and `rust_queries/`. The Rust modules then use `include_str!` instead of inline `const &str`. This is also better for maintainability — `.scm` files have editor support.

If a test fails: open the failing `.scm` file, check the offending node/field name against the grammar's `node-types.json` (`cargo doc -p tree-sitter-rust --open`), fix the query, and re-run.

- [ ] **Step 5: Run snapshot tests, approve, commit**

Run: `cargo test -p mycel-extract`
Run: `cargo insta review` (approve each)
Re-run: `cargo test -p mycel-extract`
Expected: PASS — both `query_compile` tests AND fixture snapshots.

```bash
git add crates/mycel-extract tests/fixtures/rust
git commit -m "Add RustExtractor with golden snapshot tests and query-compile guard"
```

End of Chunk 3.

---

## Chunk 4: LSP refinement (multilspy bridge)

This chunk produces the multilspy Python subprocess bridge and the Rust-side `MultilspyResolver`. By the end, calling `resolver.refine(file, extraction)` returns `Edge`s with `EdgeSource::Lsp` for both TS and Rust files.

### Architectural notes

- The bridge is a long-running Python process. The Rust side spawns it once, communicates via JSON-line over stdin/stdout, multiplexes requests by `id`.
- `edges_for_file` is a custom aggregation built in Python: documentSymbol + callHierarchy + references stitched into our edge model.
- If multilspy can't reach a language server (e.g., `tsserver` not installed), the bridge returns `{"partial": true, "edges": [...what it could resolve...]}`. Mycel-lsp logs a warning and proceeds with partial data.

### Task 4.1: Python bridge script

**Files:**
- Create: `scripts/multilspy_bridge.py`

- [ ] **Step 1: Write the bridge**

```python
#!/usr/bin/env python3
"""Long-running multilspy bridge for mycel-daemon.

Reads JSON-line requests on stdin, writes JSON-line responses on stdout.
Each request: {"id": <int>, "op": "edges_for_file", "language": "typescript"|"rust",
               "repo_root": "<abs path>", "path": "<rel path>"}.
Each response: {"id": <int>, "edges": [...], "partial": <bool>, "error": <str?>}.

Edges shape: {"from": "<qname>", "to": "<qname>", "kind": "calls"|"uses_type"|"implements"|"imports", "source": "lsp"}.

Phase 1 strategy: multilspy's documented public API surface is
`request_definition`, `request_references`, `request_document_symbols`,
`request_hover`, `request_completions` (verified against the package README).
There is NO documented `request_outgoing_calls` / `callHierarchy` method on
the SyncLanguageServer surface. This bridge therefore does NOT use call
hierarchy. Instead, tree-sitter produces tentative CALLS edges (extractor),
and this bridge only refines USES_TYPE / IMPLEMENTS / REFERENCES via
`request_definition` + `request_document_symbols`.

If/when call-hierarchy lands in multilspy, extend this bridge to upgrade
CALLS edges from `tree-sitter` to `lsp` source.
"""
import json
import sys
import traceback

try:
    from multilspy import SyncLanguageServer
    from multilspy.multilspy_config import MultilspyConfig
    from multilspy.multilspy_logger import MultilspyLogger
except ImportError as e:
    sys.stderr.write(f"multilspy not installed: {e}\n")
    sys.exit(2)

LANG_TO_MULTILSPY = {
    "typescript": "typescript",
    "rust":       "rust",
}

def emit(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()

def _selection_start(sym):
    """LSP DocumentSymbol uses `selectionRange` for the name span;
    SymbolInformation uses `location.range`. Be defensive."""
    sel = sym.get("selectionRange") or sym.get("range")
    if sel and "start" in sel:
        return sel["start"]
    loc = sym.get("location", {})
    rng = loc.get("range") or {}
    return rng.get("start")

class BridgeState:
    def __init__(self):
        self.servers = {}  # (repo_root, language) -> (server, cm)

    def get_server(self, repo_root: str, language: str):
        key = (repo_root, language)
        if key not in self.servers:
            mlang = LANG_TO_MULTILSPY[language]
            cfg = MultilspyConfig.from_dict({"code_language": mlang})
            logger = MultilspyLogger()
            server = SyncLanguageServer.create(cfg, logger, repo_root)
            cm = server.start_server()
            cm.__enter__()
            self.servers[key] = (server, cm)
        return self.servers[key][0]

    def edges_for_file(self, repo_root: str, language: str, path: str):
        """Refine extractor edges using LSP definition lookups.

        For Phase 1, the bridge produces:
        - REFERENCES edges from each documentSymbol to symbols it references
          (resolved via request_definition at each name position).

        CALLS edges are NOT produced here in v0 — multilspy's documented
        public API does not expose call hierarchy. The tree-sitter extractor
        produces heuristic CALLS edges; LSP refinement of those is deferred
        until multilspy adds call-hierarchy support.
        """
        server = self.get_server(repo_root, language)
        edges = []
        partial = False
        try:
            doc_symbols = server.request_document_symbols(path)
            # Some multilspy versions return a tuple, others a list; normalize.
            if isinstance(doc_symbols, tuple):
                doc_symbols = doc_symbols[0]
            for sym in (doc_symbols or []):
                name = sym.get("name") if isinstance(sym, dict) else None
                if not name:
                    continue
                start = _selection_start(sym)
                if not start:
                    continue
                try:
                    defs = server.request_definition(path, start["line"], start["character"])
                    for d in (defs or []):
                        target_uri = d.get("uri") or d.get("targetUri")
                        if not target_uri:
                            continue
                        edges.append({
                            "from": f"{path}::{name}",
                            "to": target_uri,
                            "kind": "references",
                            "source": "lsp",
                        })
                except Exception:
                    partial = True
        except Exception:
            partial = True
        return {"edges": edges, "partial": partial}

def main():
    state = BridgeState()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError as e:
            emit({"id": None, "error": f"bad json: {e}"})
            continue
        rid = req.get("id")
        op = req.get("op")
        try:
            if op == "edges_for_file":
                result = state.edges_for_file(
                    req["repo_root"], req["language"], req["path"]
                )
                emit({"id": rid, **result})
            elif op == "ping":
                emit({"id": rid, "pong": True})
            elif op == "shutdown":
                emit({"id": rid, "shutdown": True})
                break
            else:
                emit({"id": rid, "error": f"unknown op: {op}"})
        except Exception as e:
            tb = traceback.format_exc()
            emit({"id": rid, "error": str(e), "traceback": tb})

if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Quick manual smoke test**

```sh
echo '{"id": 1, "op": "ping"}' | python3 scripts/multilspy_bridge.py
```

Expected output: `{"id": 1, "pong": true}`

- [ ] **Step 3: Commit**

```bash
git add scripts/multilspy_bridge.py
git commit -m "Add multilspy Python bridge with JSON-line protocol"
```

### Task 4.2: `mycel-lsp` Rust-side `MultilspyResolver`

**Files:**
- Modify: `crates/mycel-lsp/src/lib.rs`
- Create: `crates/mycel-lsp/src/multilspy.rs`
- Create: `crates/mycel-lsp/src/protocol.rs`

- [ ] **Step 1: Write the failing integration test (gated behind `MYCEL_TEST_LSP=1`)**

Create `crates/mycel-lsp/tests/smoke.rs`:

```rust
//! Requires multilspy installed and tsserver available. Set MYCEL_TEST_LSP=1 to run.

use camino::Utf8PathBuf;
use mycel_core::*;
use mycel_lsp::*;

#[tokio::test]
async fn lsp_smoke_typescript() {
    if std::env::var("MYCEL_TEST_LSP").ok().as_deref() != Some("1") {
        eprintln!("skipping (set MYCEL_TEST_LSP=1 to run)");
        return;
    }
    let repo: Utf8PathBuf = std::env::current_dir().unwrap().try_into().unwrap();
    let resolver = MultilspyResolver::spawn(
        "python3 scripts/multilspy_bridge.py",
        repo.clone(),
    ).await.expect("spawn multilspy");
    let path: Utf8PathBuf = "tests/fixtures/typescript/imports_and_exports.ts".into();
    let extraction = mycel_extract::ExtractionOutput::default();
    let edges = resolver.refine(&path, "typescript", &extraction).await
        .expect("refine returns Ok even when partial");
    eprintln!("got {} edges", edges.len());
    // Real assertion: imports_and_exports.ts has at least one definition
    // (`add` from ./simple_function) that LSP should resolve. If we get zero,
    // the bridge or multilspy is broken and the test should fail loudly,
    // not pass with eprintln.
    assert!(!edges.is_empty(), "expected at least one LSP edge from a TS fixture with cross-file references");
}
```

Add `mycel-extract` and `tokio` (with `macros` and `rt-multi-thread`) to dev-deps as needed; for the test gate, this just smoke-tests that the resolver can be spawned and called.

- [ ] **Step 2: Implement `src/protocol.rs`**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct EdgesForFileReq<'a> {
    pub id: u64,
    pub op: &'static str,
    pub repo_root: &'a str,
    pub language: &'a str,
    pub path: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct EdgesForFileResp {
    pub id: u64,
    #[serde(default)]
    pub edges: Vec<RawEdge>,
    #[serde(default)]
    pub partial: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RawEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    #[serde(default)]
    pub source: Option<String>,
}
```

Note: `mycel-lsp` needs `mycel-extract` as a dep so the `refine` signature can take `&mycel_extract::ExtractionOutput`. Add `mycel-extract = { path = "../mycel-extract" }` to `crates/mycel-lsp/Cargo.toml` BEFORE the steps below.

- [ ] **Step 3a: Skeleton — struct, fields, common imports**

```rust
// crates/mycel-lsp/src/multilspy.rs
use crate::protocol::*;
use camino::Utf8PathBuf;
use mycel_core::*;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{oneshot, Mutex};
use tracing::{warn, debug};

pub struct MultilspyResolver {
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<EdgesForFileResp>>>>,
    stdin: Arc<Mutex<ChildStdin>>,
    repo_root: Utf8PathBuf,
    _child: Child,
}
```

Run `cargo build -p mycel-lsp`. Expected: clean (struct only; no impl yet).

- [ ] **Step 3b: `spawn` constructor + reader-loop wiring**

Append to `multilspy.rs`:

```rust
impl MultilspyResolver {
    pub async fn spawn(cmd: &str, repo_root: Utf8PathBuf) -> Result<Self> {
        let mut parts = cmd.split_whitespace();
        let exe = parts.next().ok_or_else(|| MycelError::Lsp("empty cmd".into()))?;
        let args: Vec<&str> = parts.collect();

        let mut child = tokio::process::Command::new(exe)
            .args(&args)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn()
            .map_err(|e| MycelError::Lsp(format!("spawn: {e}")))?;

        let stdin = child.stdin.take().ok_or_else(|| MycelError::Lsp("no stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| MycelError::Lsp("no stdout".into()))?;

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<EdgesForFileResp>>>> = Default::default();
        let p2 = pending.clone();
        tokio::spawn(reader_loop(stdout, p2));

        Ok(Self {
            next_id: AtomicU64::new(1),
            pending,
            stdin: Arc::new(Mutex::new(stdin)),
            repo_root,
            _child: child,
        })
    }
}

async fn reader_loop(
    stdout: ChildStdout,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<EdgesForFileResp>>>>,
) {
    let mut reader = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        debug!(line = %line, "bridge response");
        match serde_json::from_str::<EdgesForFileResp>(&line) {
            Ok(resp) => {
                if let Some(tx) = pending.lock().await.remove(&resp.id) {
                    let _ = tx.send(resp);
                }
            }
            Err(e) => warn!(error = %e, line = %line, "could not parse bridge response"),
        }
    }
}
```

Run `cargo build -p mycel-lsp`. Expected: clean (now spawnable but `refine` still missing).

- [ ] **Step 3c: `refine` method (request/response round-trip + edge mapping)**

Append:

```rust
impl MultilspyResolver {
    pub async fn refine(
        &self,
        path: &camino::Utf8Path,
        language: &str,
        _extraction: &mycel_extract::ExtractionOutput,
    ) -> Result<Vec<Edge>> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let req = EdgesForFileReq {
            id, op: "edges_for_file",
            repo_root: self.repo_root.as_str(),
            language, path: path.as_str(),
        };
        let line = serde_json::to_string(&req)? + "\n";
        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await
                .map_err(|e| MycelError::Lsp(format!("write: {e}")))?;
            stdin.flush().await.map_err(|e| MycelError::Lsp(format!("flush: {e}")))?;
        }
        let resp = rx.await.map_err(|_| MycelError::Lsp("bridge closed".into()))?;
        if let Some(err) = resp.error {
            warn!(language, %path, "LSP bridge error: {err}");
            return Ok(Vec::new());
        }
        if resp.partial {
            warn!(language, %path, "LSP refinement returned partial results");
        }
        Ok(resp.edges.into_iter().filter_map(|r| {
            let kind = match r.kind.as_str() {
                "calls" => EdgeKind::Calls,
                "uses_type" => EdgeKind::UsesType,
                "implements" => EdgeKind::Implements,
                "imports" => EdgeKind::Imports,
                "references" => EdgeKind::References,
                _ => return None,
            };
            Some(Edge { from: r.from, to: r.to, kind, source: EdgeSource::Lsp })
        }).collect())
    }
}
```

Run `cargo build -p mycel-lsp`. Expected: clean (resolver now functional).

- [ ] **Step 4: Wire `src/lib.rs`**

```rust
//! mycel-lsp — Phase 1 ships only the multilspy-backed resolver.
//!
//! A `Resolver` trait abstraction is intentionally NOT defined here. Phase 1
//! has exactly one resolver implementation; introducing a trait for one impl
//! is premature. When a second resolver lands (native Rust LSP client, or a
//! mock for testing without Python), extract the trait at that point. Until
//! then, callers depend on `MultilspyResolver` directly.

pub mod multilspy;
pub mod protocol;

pub use multilspy::MultilspyResolver;
```

- [ ] **Step 5: Run smoke test (gated)**

Run: `MYCEL_TEST_LSP=1 cargo test -p mycel-lsp -- --nocapture`
Expected: PASS or "skipping" if multilspy/tsserver not installed.

- [ ] **Step 6: Commit**

```bash
git add crates/mycel-lsp
git commit -m "Add MultilspyResolver and bridge protocol types"
```

End of Chunk 4.

---

## Chunk 5: Model providers (Ollama embedder + trait stubs)

This chunk produces `mycel-models` with the three trait shapes (`Embedder` / `Synthesizer` / `Reranker`) and a working `OllamaEmbedder` that hits Ollama's `/api/embed` endpoint. `OllamaSynthesizer` and `OllamaReranker` exist as `todo!()` stubs — Phase 1 doesn't call them but the trait surface is locked.

### Architectural note: float-comparison trap

When testing `Symbol` round-trips that include `embedding: Some(Vec<f32>)` (later phases), the `Symbol`-derived `PartialEq` uses `f32::PartialEq`, which has IEEE 754 NaN semantics. Tests that compare embeddings directly will silently fail on NaN. Use approximate comparison (e.g., a helper that tolerates `eps`) when writing such tests.

### Task 5.1: Trait surface

**Files:**
- Modify: `crates/mycel-models/src/lib.rs`

- [ ] **Step 1: Define traits**

```rust
use async_trait::async_trait;
use mycel_core::Result;

#[async_trait]
pub trait Embedder: Send + Sync {
    /// Stable provider+model identity, e.g., "ollama/embeddinggemma".
    fn identity(&self) -> &str;
    /// Output vector dimension.
    fn dimension(&self) -> u32;
    /// Embed a batch of texts.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

#[async_trait]
pub trait Synthesizer: Send + Sync {
    fn identity(&self) -> &str;
    async fn synthesize(&self, prompt: &str) -> Result<String>;
}

#[async_trait]
pub trait Reranker: Send + Sync {
    fn identity(&self) -> &str;
    /// Score each candidate against the query. Higher = more relevant.
    async fn rerank(&self, query: &str, candidates: &[&str]) -> Result<Vec<f32>>;
}

pub mod ollama;
pub use ollama::OllamaEmbedder;
```

Add `async-trait = "0.1"` to `mycel-models/Cargo.toml` deps and to `[workspace.dependencies]`.

### Task 5.2: `OllamaEmbedder` against `/api/embed`

**Files:**
- Create: `crates/mycel-models/src/ollama.rs`

- [ ] **Step 1: Write the failing integration test (gated behind running Ollama)**

Create `crates/mycel-models/tests/embed.rs`:

```rust
use mycel_models::*;

#[tokio::test]
async fn ollama_embed_smoke() {
    if std::env::var("MYCEL_TEST_OLLAMA").ok().as_deref() != Some("1") {
        eprintln!("skipping (set MYCEL_TEST_OLLAMA=1 to run; requires `ollama pull embeddinggemma`)");
        return;
    }
    let e = OllamaEmbedder::new("http://localhost:11434", "embeddinggemma");
    let out = e.embed(&["the quick brown fox", "lazy dog"]).await.unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].len(), e.dimension() as usize);
}
```

- [ ] **Step 2: Implement `src/ollama.rs`**

```rust
use crate::{Embedder, Synthesizer, Reranker};
use async_trait::async_trait;
use mycel_core::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub struct OllamaEmbedder {
    endpoint: String,
    model: String,
    dimension: u32,
    identity: String,
    client: reqwest::Client,
}

impl OllamaEmbedder {
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>) -> Self {
        let model = model.into();
        let identity = format!("ollama/{model}");
        // EmbeddingGemma defaults to 768d.
        Self {
            endpoint: endpoint.into(),
            model,
            dimension: 768,
            identity,
            client: reqwest::Client::new(),
        }
    }
    pub fn with_dimension(mut self, d: u32) -> Self { self.dimension = d; self }
}

#[derive(Serialize)]
struct EmbedRequest<'a> { model: &'a str, input: &'a [&'a str] }

#[derive(Deserialize)]
struct EmbedResponse { embeddings: Vec<Vec<f32>> }

#[async_trait]
impl Embedder for OllamaEmbedder {
    fn identity(&self) -> &str { &self.identity }
    fn dimension(&self) -> u32 { self.dimension }
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/api/embed", self.endpoint);
        let body = json!({"model": self.model, "input": texts});
        let resp: EmbedResponse = self.client.post(&url)
            .json(&body)
            .send().await
            .map_err(|e| MycelError::Model { provider: self.identity.clone(), message: format!("send: {e}") })?
            .error_for_status()
            .map_err(|e| MycelError::Model { provider: self.identity.clone(), message: format!("status: {e}") })?
            .json().await
            .map_err(|e| MycelError::Model { provider: self.identity.clone(), message: format!("json: {e}") })?;
        // Validate dimension matches the embedder's declared dimension. If
        // the model returns a different dimension, fail loudly here rather
        // than letting a corrupt vector reach the FalkorDB vector index.
        if let Some(first) = resp.embeddings.first() {
            if first.len() as u32 != self.dimension {
                return Err(MycelError::Model {
                    provider: self.identity.clone(),
                    message: format!(
                        "embedding dimension mismatch: model returned {} but embedder declares {}",
                        first.len(), self.dimension
                    ),
                });
            }
        }
        Ok(resp.embeddings)
    }
}

pub struct OllamaSynthesizer { pub endpoint: String, pub model: String }
#[async_trait]
impl Synthesizer for OllamaSynthesizer {
    fn identity(&self) -> &str { &self.model }
    async fn synthesize(&self, _prompt: &str) -> Result<String> {
        todo!("phase 2 — wire generate endpoint")
    }
}

pub struct OllamaReranker { pub endpoint: String, pub model: String }
#[async_trait]
impl Reranker for OllamaReranker {
    fn identity(&self) -> &str { &self.model }
    async fn rerank(&self, _query: &str, _candidates: &[&str]) -> Result<Vec<f32>> {
        todo!("phase 3 — wire reranker endpoint")
    }
}
```

- [ ] **Step 3: Run integration test**

Run: `MYCEL_TEST_OLLAMA=1 cargo test -p mycel-models`
Expected: PASS (after `ollama pull embeddinggemma`).

- [ ] **Step 4: Commit**

```bash
git add crates/mycel-models
git commit -m "Add Embedder/Synthesizer/Reranker traits and OllamaEmbedder"
```

End of Chunk 5.

---

## Chunk 6: Indexing pipeline

`mycel-index::Indexer` orchestrates the Phase 1 pipeline: parse → refine (LSP) → graph upsert → embed signatures. Content-hash dedup skips files that haven't changed.

### Task 6.1: Indexer with content-hash dedup

**Files:**
- Modify: `crates/mycel-index/src/lib.rs`
- Create: `crates/mycel-index/src/pipeline.rs`
- Create: `crates/mycel-index/src/dedup.rs`

- [ ] **Step 0: Add `File`-record + symbol-embedding methods to `mycel-graph`**

Per the workspace convention "**`mycel-graph`** — Sole owner of Cypher and FalkorDB. No other crate touches them," these helpers belong in `mycel-graph`, not in `mycel-index`. Add them in a new file `crates/mycel-graph/src/file.rs`:

```rust
use crate::{GraphClient, symbol};
use falkordb::FalkorValue;
use camino::Utf8Path;
use mycel_core::*;

impl GraphClient {
    /// Returns the stored content_hash for a File node, if any.
    pub async fn file_content_hash(&self, path: &Utf8Path) -> Result<Option<String>> {
        let cypher = format!(
            "MATCH (f:File {{path: '{}'}}) RETURN f.content_hash",
            symbol::escape(path.as_str())
        );
        let rows = self.query(&cypher).await?;
        Ok(rows.into_iter().next()
            .and_then(|r| r.into_iter().next())
            .and_then(|v| match v { FalkorValue::String(s) => Some(s), _ => None }))
    }

    pub async fn upsert_file_record(
        &self, path: &Utf8Path, language: &str, content_hash: &str,
    ) -> Result<()> {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let cypher = format!(
            "MERGE (f:File {{path: '{path}'}}) SET f.language='{lang}', f.content_hash='{hash}', f.last_modified={ts}",
            path = symbol::escape(path.as_str()),
            lang = symbol::escape(language),
            hash = symbol::escape(content_hash),
            ts = now,
        );
        self.query(&cypher).await?;
        Ok(())
    }

    /// Sets the embedding vector property on a Symbol node.
    pub async fn set_symbol_embedding(&self, qualified_name: &str, vec: &[f32]) -> Result<()> {
        let vec_lit = vec.iter().map(|f| f.to_string()).collect::<Vec<_>>().join(",");
        let cypher = format!(
            "MATCH (s:Symbol {{qualified_name: '{q}'}}) SET s.embedding = vecf32([{v}])",
            q = symbol::escape(qualified_name),
            v = vec_lit,
        );
        self.query(&cypher).await?;
        Ok(())
    }
}
```

Wire `pub mod file;` into `crates/mycel-graph/src/lib.rs`. Run `cargo build -p mycel-graph` to confirm clean.

- [ ] **Step 1: Write `src/dedup.rs` (uses `mycel-graph` typed methods only)**

```rust
use blake3::Hasher;
use camino::Utf8Path;
use mycel_core::Result;
use mycel_graph::GraphClient;

pub fn content_hash(content: &str) -> String {
    let mut h = Hasher::new();
    h.update(content.as_bytes());
    h.finalize().to_hex().to_string()
}

/// True if the file's stored hash equals `hash`.
pub async fn is_unchanged(client: &GraphClient, path: &Utf8Path, hash: &str) -> Result<bool> {
    Ok(client.file_content_hash(path).await? == Some(hash.to_string()))
}

pub async fn upsert_file_record(
    client: &GraphClient, path: &Utf8Path, language: &str, hash: &str,
) -> Result<()> {
    client.upsert_file_record(path, language, hash).await
}
```

- [ ] **Step 2: Write `src/pipeline.rs`**

```rust
use camino::Utf8Path;
use mycel_core::*;
use mycel_extract::for_language;
use mycel_graph::GraphClient;
use mycel_lsp::MultilspyResolver;
use mycel_models::Embedder;
use std::sync::Arc;
use tracing::{info, warn, debug};

pub struct Indexer {
    pub graph: GraphClient,
    pub lsp: Option<Arc<MultilspyResolver>>,
    pub embedder: Arc<dyn Embedder>,
}

impl Indexer {
    pub async fn index_file(&self, path: &Utf8Path, content: &str) -> Result<()> {
        // 1. Content-hash dedup
        let hash = crate::dedup::content_hash(content);
        if crate::dedup::is_unchanged(&self.graph, path, &hash).await? {
            debug!(file=%path, "unchanged, skipping");
            return Ok(());
        }

        // 2. Pick extractor
        let Some(extractor) = for_language(path) else {
            debug!(file=%path, "no extractor for extension");
            return Ok(());
        };
        let extraction = extractor.extract(path, content)?;
        info!(file=%path, n_symbols = extraction.symbols.len(), n_edges = extraction.edges.len(), "extracted");

        // 3. LSP refinement (if configured)
        let mut all_edges = extraction.edges.clone();
        if let Some(lsp) = &self.lsp {
            match lsp.refine(path, extractor.language_name(), &extraction).await {
                Ok(lsp_edges) => {
                    debug!(file=%path, n_lsp_edges = lsp_edges.len(), "lsp refined");
                    all_edges.extend(lsp_edges);
                }
                Err(e) => warn!(file=%path, error=%e, "lsp refine failed; proceeding with tree-sitter only"),
            }
        }

        // 4. Graph upsert
        self.graph.upsert_symbol_batch(&extraction.symbols).await?;
        self.graph.upsert_edge_batch(&all_edges).await?;

        // 5. Embed signatures (Phase 1: signature-only).
        //
        // CRITICAL: pair each symbol with its OWN embedding. The chunk-of-32
        // batching means we must zip the symbol-chunk with the text-chunk,
        // not zip `extraction.symbols.iter()` (which always restarts at 0)
        // with the most recent batch's vectors.
        if !extraction.symbols.is_empty() {
            let texts: Vec<&str> = extraction.symbols.iter()
                .map(|s| s.signature.as_str()).collect();
            for (sym_chunk, text_chunk) in extraction.symbols.chunks(32).zip(texts.chunks(32)) {
                let vecs = self.embedder.embed(text_chunk).await?;
                if vecs.len() != sym_chunk.len() {
                    return Err(MycelError::Model {
                        provider: self.embedder.identity().into(),
                        message: format!("expected {} embeddings, got {}", sym_chunk.len(), vecs.len()),
                    });
                }
                for (sym, vec) in sym_chunk.iter().zip(vecs.iter()) {
                    self.graph.set_symbol_embedding(sym.qualified_name.as_str(), vec).await?;
                }
            }
        }

        // 6. Update File record
        crate::dedup::upsert_file_record(&self.graph, path, extractor.language_name(), &hash).await?;
        Ok(())
    }

    pub async fn index_repo(&self, root: &Utf8Path) -> Result<usize> {
        let mut count = 0;
        for entry in walkdir::WalkDir::new(root)
            .into_iter().filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() { continue; }
            let path: camino::Utf8PathBuf = entry.path().to_path_buf().try_into()
                .map_err(|_| MycelError::Extract { file: entry.path().display().to_string(), message: "non-utf8 path".into() })?;
            // skip target/, node_modules/, .git/
            if path.as_str().contains("/target/") || path.as_str().contains("/node_modules/") || path.as_str().contains("/.git/") {
                continue;
            }
            if for_language(&path).is_none() { continue; }
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    if let Err(e) = self.index_file(&path, &content).await {
                        warn!(file=%path, error=%e, "index_file failed");
                    } else {
                        count += 1;
                    }
                }
                Err(e) => warn!(file=%path, error=%e, "read failed"),
            }
        }
        Ok(count)
    }
}
```

Add `walkdir = "2"` to `mycel-index` Cargo.toml deps.

- [ ] **Step 3: Wire `src/lib.rs`**

```rust
pub mod dedup;
pub mod pipeline;

pub use pipeline::Indexer;
```

- [ ] **Step 4: Run `cargo build -p mycel-index`; verify it compiles**

Run: `cargo build -p mycel-index`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-index
git commit -m "Add Indexer with content-hash dedup and Phase 1 pipeline"
```

End of Chunk 6.

---

## Chunk 7: Query layer + CLI + supervisor

`mycel-query` implements the actual query logic for each command. `mycel-cli` is the binary with clap subcommands and the cross-platform supervisor module.

### Task 7.1: `mycel-query` Tier 1 + signature-only `find`

**Files:**
- Modify: `crates/mycel-query/src/lib.rs`
- Create: `crates/mycel-query/src/find.rs`

- [ ] **Step 1: Write `src/lib.rs`**

```rust
//! Query implementations called by the mycel CLI.

pub mod find;

use mycel_core::*;
use mycel_graph::GraphClient;

pub async fn callers(g: &GraphClient, sym: &str) -> Result<Vec<Symbol>>     { g.query_callers(sym).await }
pub async fn callees(g: &GraphClient, sym: &str) -> Result<Vec<Symbol>>     { g.query_callees(sym).await }
pub async fn definers(g: &GraphClient, name: &str) -> Result<Vec<Symbol>>   { g.query_definers(name).await }
pub async fn imports(g: &GraphClient, file: &str) -> Result<Vec<Symbol>>    { g.query_imports(file).await }
pub async fn uses(g: &GraphClient, ty: &str) -> Result<Vec<Symbol>>         { g.query_uses(ty).await }
pub async fn implements(g: &GraphClient, iface: &str) -> Result<Vec<Symbol>>{ g.query_implements(iface).await }

pub use find::find;
```

- [ ] **Step 2: Write `src/find.rs`**

```rust
use mycel_core::*;
use mycel_graph::GraphClient;
use mycel_models::Embedder;

pub async fn find(
    g: &GraphClient,
    embedder: &dyn Embedder,
    query: &str,
    limit: usize,
) -> Result<Vec<(Symbol, f32)>> {
    let mut vecs = embedder.embed(&[query]).await?;
    let v = vecs.pop()
        .ok_or_else(|| MycelError::Model { provider: embedder.identity().into(), message: "empty embedding".into() })?;
    g.vector_search_top_k(&v, limit, embedder.identity(), embedder.dimension()).await
}
```

- [ ] **Step 3: Verify build**

Run: `cargo build -p mycel-query`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/mycel-query
git commit -m "Add Tier 1 queries and signature-only mycel find"
```

### Task 7.2: `mycel-cli` clap subcommands + output formatting

**Files:**
- Modify: `crates/mycel-cli/src/main.rs`
- Create: `crates/mycel-cli/src/output.rs`
- Create: `crates/mycel-cli/src/config.rs`

- [ ] **Step 1: Write `src/config.rs`**

```rust
//! Layered config loading: defaults -> ~/.config/mycel/config.toml -> <repo>/.mycel.toml -> env.
//!
//! Layered values use `Option<T>` to track presence; only `Some(...)` values
//! from later layers replace earlier ones. Defaults are applied LAST when
//! materializing the resolved config — so a `Default::default()` field on an
//! intermediate layer never accidentally beats a real user/repo value.

use mycel_core::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigLayer {
    #[serde(default)] pub models: Option<ModelsConfig>,
    #[serde(default)] pub storage: Option<StorageOverlay>,
    #[serde(default)] pub providers: Option<ProvidersOverlay>,
    #[serde(default)] pub lsp: Option<LspConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageOverlay {
    #[serde(default)] pub falkordb_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProvidersOverlay {
    #[serde(default)] pub embedder: Option<ProviderConfig>,
    #[serde(default)] pub synthesizer: Option<ProviderConfig>,
    #[serde(default)] pub reranker: Option<ProviderConfig>,
}

/// The fully resolved config used by the CLI/daemon. All fields are populated.
#[derive(Debug, Clone)]
pub struct Config {
    pub models: ModelsConfig,
    pub storage: StorageConfig,
    pub providers: ProvidersConfig,
    pub lsp: LspConfig,
}

fn merge(into: &mut ConfigLayer, from: ConfigLayer) {
    if from.models.is_some() { into.models = from.models; }
    if let Some(s) = from.storage {
        let target = into.storage.get_or_insert_with(StorageOverlay::default);
        if s.falkordb_url.is_some() { target.falkordb_url = s.falkordb_url; }
    }
    if let Some(p) = from.providers {
        let target = into.providers.get_or_insert_with(ProvidersOverlay::default);
        if p.embedder.is_some()    { target.embedder    = p.embedder; }
        if p.synthesizer.is_some() { target.synthesizer = p.synthesizer; }
        if p.reranker.is_some()    { target.reranker    = p.reranker; }
    }
    if from.lsp.is_some() { into.lsp = from.lsp; }
}

pub fn load(repo: Option<&camino::Utf8Path>) -> Result<Config> {
    let mut layered = ConfigLayer::default();

    // user-global
    if let Some(home) = std::env::var_os("HOME") {
        let home: camino::Utf8PathBuf = camino::Utf8PathBuf::from_path_buf(home.into())
            .map_err(|_| MycelError::Config("non-utf8 HOME".into()))?;
        let user = home.join(".config/mycel/config.toml");
        if user.exists() {
            let s = std::fs::read_to_string(&user)?;
            let parsed: ConfigLayer = toml::from_str(&s)
                .map_err(|e| MycelError::Config(format!("user config: {e}")))?;
            merge(&mut layered, parsed);
        }
    }

    // per-repo
    if let Some(repo) = repo {
        let repo_cfg = repo.join(".mycel.toml");
        if repo_cfg.exists() {
            let s = std::fs::read_to_string(&repo_cfg)?;
            let parsed: ConfigLayer = toml::from_str(&s)
                .map_err(|e| MycelError::Config(format!("repo config: {e}")))?;
            merge(&mut layered, parsed);
        }
    }

    // env overrides (highest precedence)
    if let Ok(url) = std::env::var("MYCEL_FALKORDB_URL") {
        layered.storage.get_or_insert_with(StorageOverlay::default).falkordb_url = Some(url);
    }

    // materialize with defaults last
    let storage = StorageConfig {
        falkordb_url: layered.storage.and_then(|s| s.falkordb_url)
            .unwrap_or_else(|| "redis://localhost:6379".into()),
    };
    let providers_overlay = layered.providers.unwrap_or_default();
    let providers = ProvidersConfig {
        embedder:    providers_overlay.embedder,
        synthesizer: providers_overlay.synthesizer,
        reranker:    providers_overlay.reranker,
    };
    Ok(Config {
        models: layered.models.unwrap_or_default(),
        storage,
        providers,
        lsp: layered.lsp.unwrap_or_default(),
    })
}

pub fn embedder_from_cfg(cfg: &Config) -> std::sync::Arc<dyn mycel_models::Embedder> {
    let provider = cfg.providers.embedder.clone().unwrap_or(ProviderConfig::default_ollama());
    match provider {
        ProviderConfig::Ollama { endpoint, model, .. } => {
            let model = model.unwrap_or_else(|| "embeddinggemma".into());
            std::sync::Arc::new(mycel_models::OllamaEmbedder::new(endpoint, model))
        }
    }
}
```

- [ ] **Step 2: Write `src/output.rs`**

```rust
use mycel_core::*;
use serde::Serialize;

#[derive(Serialize)]
pub struct FindHit<'a> {
    pub symbol: &'a Symbol,
    pub score: f32,
}

pub fn print_symbols(syms: &[Symbol], json: bool) {
    if json {
        println!("{}", serde_json::to_string(syms).unwrap());
    } else {
        for s in syms {
            println!("{}  {:?}  {}:{}-{}",
                s.qualified_name.as_str(), s.kind,
                s.file_path, s.start_line, s.end_line);
            println!("    {}", s.signature.as_str());
        }
    }
}

pub fn print_find(hits: &[(Symbol, f32)], json: bool) {
    if json {
        let v: Vec<_> = hits.iter().map(|(s, sc)| FindHit { symbol: s, score: *sc }).collect();
        println!("{}", serde_json::to_string(&v).unwrap());
    } else {
        for (s, score) in hits {
            println!("{:.3}  {}", score, s.qualified_name.as_str());
            println!("    {}:{}-{}  {}", s.file_path, s.start_line, s.end_line, s.signature.as_str());
        }
    }
}
```

- [ ] **Step 3a: Create `src/cli.rs` with the clap definitions**

The clap enums must be reachable from both `main.rs` and `supervisor/`. Putting them in a dedicated module is cleaner than re-exporting through `main.rs`.

```rust
// crates/mycel-cli/src/cli.rs
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mycel", version)]
pub struct Cli {
    /// Output as JSON
    #[arg(long, global = true)]
    pub json: bool,
    /// Repo root for resolving .mycel.toml
    #[arg(long, global = true)]
    pub repo: Option<Utf8PathBuf>,
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    Index { path: Utf8PathBuf },
    Callers { symbol: String },
    Callees { symbol: String },
    Definers { name: String },
    Imports { file: String },
    Uses { ty: String },
    Implements { iface: String },
    Find { query: String, #[arg(long, default_value_t = 8)] limit: usize },
    Daemon { #[command(subcommand)] action: DaemonAction },
}

#[derive(Subcommand)]
pub enum DaemonAction {
    Install,
    Uninstall,
    Start,
    Stop,
    Status,
    Logs { #[arg(long)] follow: bool },
    Run,
}
```

- [ ] **Step 3b: Write `src/main.rs`**

```rust
mod cli;
mod config;
mod output;
mod supervisor;

use anyhow::Context;
use camino::Utf8PathBuf;
use clap::Parser;
use cli::{Cli, Cmd};
use mycel_graph::GraphClient;
use mycel_index::Indexer;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let cli = Cli::parse();
    let cfg = config::load(cli.repo.as_deref())?;
    let json = cli.json;

    match cli.command {
        Cmd::Index { path } => {
            let graph_name = format!("mycel:{}", repo_id_from_path(&path));
            let g = GraphClient::connect(&cfg.storage.falkordb_url, &graph_name).await?;
            let embedder = config::embedder_from_cfg(&cfg);
            // LSP refinement runs even from `mycel index` so Tier 1 queries
            // return correct results (CALLS edges with `source: lsp` are the
            // authoritative ones; tree-sitter heuristics alone miss too many
            // cases). First-run is slower as a result; that's acceptable for
            // the v0 dogfood loop.
            let canon: Utf8PathBuf = path.canonicalize_utf8().unwrap_or(path.clone());
            let lsp = match mycel_lsp::MultilspyResolver::spawn(
                &cfg.lsp.multilspy_path, canon.clone()
            ).await {
                Ok(r) => Some(Arc::new(r)),
                Err(e) => {
                    eprintln!("LSP unavailable, falling back to tree-sitter only: {e}");
                    None
                }
            };
            let indexer = Indexer { graph: g, lsp, embedder };
            let n = indexer.index_repo(&path).await?;
            println!("indexed {n} files");
        }
        Cmd::Callers { symbol } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::callers(&g, &symbol).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Callees { symbol } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::callees(&g, &symbol).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Definers { name } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::definers(&g, &name).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Imports { file } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::imports(&g, &file).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Uses { ty } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::uses(&g, &ty).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Implements { iface } => {
            let g = open(&cfg, &cli.repo).await?;
            let r = mycel_query::implements(&g, &iface).await?;
            output::print_symbols(&r, json);
        }
        Cmd::Find { query, limit } => {
            let g = open(&cfg, &cli.repo).await?;
            let embedder = config::embedder_from_cfg(&cfg);
            let r = mycel_query::find(&g, embedder.as_ref(), &query, limit).await?;
            output::print_find(&r, json);
        }
        Cmd::Daemon { action } => supervisor::dispatch(action).await?,
    }
    Ok(())
}

/// Stable repo id derived from the canonical path, so two repos with the
/// same final component (e.g. ~/work/foo and ~/personal/foo) don't collide.
/// Format: `<basename>-<8 hex chars of blake3(canonical path)>`.
fn repo_id_from_path(path: &camino::Utf8Path) -> String {
    let canon = path.canonicalize_utf8().unwrap_or_else(|_| path.to_path_buf());
    let basename = canon.file_name().unwrap_or("repo");
    let hash = blake3::hash(canon.as_str().as_bytes()).to_hex();
    format!("{basename}-{}", &hash.as_str()[..8])
}

async fn open(cfg: &config::Config, repo: &Option<Utf8PathBuf>) -> anyhow::Result<GraphClient> {
    let graph_name = format!("mycel:{}", repo.as_deref()
        .map(repo_id_from_path)
        .unwrap_or_else(|| "default".into()));
    GraphClient::connect(&cfg.storage.falkordb_url, &graph_name).await
        .context("connect to FalkorDB")
}
```

- [ ] **Step 4: Confirm build**

Run: `cargo build -p mycel-cli`
Expected: clean (the supervisor module hasn't been written yet — add a stub).

- [ ] **Step 5: Stub `src/supervisor/mod.rs`**

Create `crates/mycel-cli/src/supervisor/mod.rs`:

```rust
use crate::cli::DaemonAction;
use anyhow::Result;

pub async fn dispatch(_action: DaemonAction) -> Result<()> {
    anyhow::bail!("supervisor not yet implemented (Task 7.3)")
}
```

Re-run build: `cargo build -p mycel-cli`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/mycel-cli
git commit -m "Add mycel CLI with clap subcommands and JSON output"
```

### Task 7.3: Cross-platform daemon supervisor

**Files:**
- Create: `crates/mycel-cli/src/supervisor/launchd.rs`
- Create: `crates/mycel-cli/src/supervisor/systemd.rs`
- Create: `crates/mycel-cli/src/supervisor/launchd.plist.tmpl`
- Create: `crates/mycel-cli/src/supervisor/mycel.service.tmpl`
- Modify: `crates/mycel-cli/src/supervisor/mod.rs`

- [ ] **Step 1: Write the launchd plist template**

`crates/mycel-cli/src/supervisor/launchd.plist.tmpl`:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.gav.mycel</string>
    <key>ProgramArguments</key>
    <array>
        <string>__BINARY_PATH__</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>__LOG_PATH__</string>
    <key>StandardErrorPath</key><string>__LOG_PATH__</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin</string>
    </dict>
</dict>
</plist>
```

- [ ] **Step 2: Write the systemd unit template**

`crates/mycel-cli/src/supervisor/mycel.service.tmpl`:
```ini
[Unit]
Description=Mycelium code intelligence daemon
After=docker.service

[Service]
Type=simple
ExecStart=__BINARY_PATH__ run
Restart=on-failure
RestartSec=5
StandardOutput=append:__LOG_PATH__
StandardError=append:__LOG_PATH__
Environment=PATH=/usr/local/bin:/usr/bin:/bin

[Install]
WantedBy=default.target
```

- [ ] **Step 3: Write `src/supervisor/launchd.rs`**

```rust
#![cfg(target_os = "macos")]
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

const TEMPLATE: &str = include_str!("launchd.plist.tmpl");

fn home() -> Result<PathBuf> { Ok(dirs::home_dir().context("no home")?) }
fn plist_path() -> Result<PathBuf> { Ok(home()?.join("Library/LaunchAgents/com.gav.mycel.plist")) }
fn log_path() -> Result<PathBuf> {
    let p = home()?.join(".cache/mycel/daemon.log");
    if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
    Ok(p)
}
fn binary_path() -> Result<String> {
    let cur = std::env::current_exe()?;
    Ok(cur.with_file_name("mycel-daemon").to_string_lossy().into_owned())
}

pub fn install() -> Result<()> {
    let plist = TEMPLATE
        .replace("__BINARY_PATH__", &binary_path()?)
        .replace("__LOG_PATH__", &log_path()?.to_string_lossy());
    std::fs::write(plist_path()?, plist)?;
    let status = Command::new("launchctl").args(["load", plist_path()?.to_str().unwrap()]).status()?;
    anyhow::ensure!(status.success(), "launchctl load failed");
    println!("installed and loaded");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let _ = Command::new("launchctl").args(["unload", plist_path()?.to_str().unwrap()]).status();
    let _ = std::fs::remove_file(plist_path()?);
    println!("uninstalled");
    Ok(())
}

pub fn start() -> Result<()> {
    let status = Command::new("launchctl").args(["start", "com.gav.mycel"]).status()?;
    anyhow::ensure!(status.success(), "launchctl start failed");
    Ok(())
}

pub fn stop() -> Result<()> {
    let status = Command::new("launchctl").args(["stop", "com.gav.mycel"]).status()?;
    anyhow::ensure!(status.success(), "launchctl stop failed");
    Ok(())
}

pub fn status() -> Result<()> {
    Command::new("launchctl").args(["list", "com.gav.mycel"]).status()?;
    Ok(())
}
```

- [ ] **Step 4: Write `src/supervisor/systemd.rs`**

```rust
#![cfg(target_os = "linux")]
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

const TEMPLATE: &str = include_str!("mycel.service.tmpl");

fn home() -> Result<PathBuf> { Ok(dirs::home_dir().context("no home")?) }
fn unit_path() -> Result<PathBuf> {
    let p = home()?.join(".config/systemd/user/mycel.service");
    if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
    Ok(p)
}
fn log_path() -> Result<PathBuf> {
    let p = home()?.join(".cache/mycel/daemon.log");
    if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
    Ok(p)
}
fn binary_path() -> Result<String> {
    let cur = std::env::current_exe()?;
    Ok(cur.with_file_name("mycel-daemon").to_string_lossy().into_owned())
}

pub fn install() -> Result<()> {
    let unit = TEMPLATE
        .replace("__BINARY_PATH__", &binary_path()?)
        .replace("__LOG_PATH__", &log_path()?.to_string_lossy());
    std::fs::write(unit_path()?, unit)?;
    Command::new("systemctl").args(["--user", "daemon-reload"]).status()?;
    Command::new("systemctl").args(["--user", "enable", "--now", "mycel.service"]).status()?;
    println!("installed and started");
    Ok(())
}
pub fn uninstall() -> Result<()> {
    Command::new("systemctl").args(["--user", "disable", "--now", "mycel.service"]).status()?;
    let _ = std::fs::remove_file(unit_path()?);
    Command::new("systemctl").args(["--user", "daemon-reload"]).status()?;
    println!("uninstalled");
    Ok(())
}
pub fn start() -> Result<()> { Command::new("systemctl").args(["--user", "start", "mycel.service"]).status()?; Ok(()) }
pub fn stop() -> Result<()>  { Command::new("systemctl").args(["--user", "stop", "mycel.service"]).status()?; Ok(()) }
pub fn status() -> Result<()> {
    Command::new("systemctl").args(["--user", "status", "mycel.service"]).status()?;
    Ok(())
}
```

- [ ] **Step 5: Wire `src/supervisor/mod.rs`**

```rust
use crate::cli::DaemonAction;
use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

#[cfg(target_os = "macos")]
mod launchd;
#[cfg(target_os = "linux")]
mod systemd;

pub async fn dispatch(action: DaemonAction) -> Result<()> {
    match action {
        DaemonAction::Install   => platform_install(),
        DaemonAction::Uninstall => platform_uninstall(),
        DaemonAction::Start     => platform_start(),
        DaemonAction::Stop      => platform_stop(),
        DaemonAction::Status    => platform_status(),
        DaemonAction::Logs { follow } => tail_logs(follow),
        DaemonAction::Run       => run_foreground().await,
    }
}

#[cfg(target_os = "macos")]   fn platform_install() -> Result<()>   { launchd::install() }
#[cfg(target_os = "linux")]   fn platform_install() -> Result<()>   { systemd::install() }
#[cfg(not(any(target_os="macos", target_os="linux")))] fn platform_install() -> Result<()> { anyhow::bail!("supervisor unsupported on this platform — use `mycel daemon run`") }

#[cfg(target_os = "macos")]   fn platform_uninstall() -> Result<()> { launchd::uninstall() }
#[cfg(target_os = "linux")]   fn platform_uninstall() -> Result<()> { systemd::uninstall() }
#[cfg(not(any(target_os="macos", target_os="linux")))] fn platform_uninstall() -> Result<()> { Ok(()) }

#[cfg(target_os = "macos")]   fn platform_start() -> Result<()> { launchd::start() }
#[cfg(target_os = "linux")]   fn platform_start() -> Result<()> { systemd::start() }
#[cfg(not(any(target_os="macos", target_os="linux")))] fn platform_start() -> Result<()> { anyhow::bail!("use `mycel daemon run`") }

#[cfg(target_os = "macos")]   fn platform_stop() -> Result<()> { launchd::stop() }
#[cfg(target_os = "linux")]   fn platform_stop() -> Result<()> { systemd::stop() }
#[cfg(not(any(target_os="macos", target_os="linux")))] fn platform_stop() -> Result<()> { Ok(()) }

#[cfg(target_os = "macos")]   fn platform_status() -> Result<()> { launchd::status() }
#[cfg(target_os = "linux")]   fn platform_status() -> Result<()> { systemd::status() }
#[cfg(not(any(target_os="macos", target_os="linux")))] fn platform_status() -> Result<()> { Ok(()) }

fn log_path() -> Result<PathBuf> {
    Ok(dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home"))?.join(".cache/mycel/daemon.log"))
}

fn tail_logs(follow: bool) -> Result<()> {
    let p = log_path()?;
    let mut cmd = Command::new("tail");
    if follow { cmd.arg("-f"); }
    cmd.arg(&p);
    cmd.status()?;
    Ok(())
}

async fn run_foreground() -> Result<()> {
    // Exec the daemon binary in the same process group; it handles signals.
    let cur = std::env::current_exe()?;
    let bin = cur.with_file_name("mycel-daemon");
    let status = Command::new(&bin).status()?;
    anyhow::ensure!(status.success(), "mycel-daemon exited with {status}");
    Ok(())
}
```

Add `dirs = "5"` to `mycel-cli` Cargo.toml deps.

Make `DaemonAction` accessible to the supervisor module (it's currently in `main.rs`):
- In `main.rs`, add `pub use DaemonAction as DaemonActionPub;` or move the enum into a module the supervisor imports. Simplest: add `pub mod main_actions { pub use super::DaemonAction; }` and use `crate::DaemonAction` from supervisor.

- [ ] **Step 6: Build + smoke install on macOS**

Run: `cargo build -p mycel-cli`
Expected: clean on macOS and Linux (cfg-gated branches don't cross-compile each other).

On macOS dev machine: `cargo run -p mycel-cli -- daemon install`
Expected: writes plist, loads it. (Note: `mycel-daemon` doesn't fully work yet — supervisor will fail to keep it alive. Uninstall after smoke check.)

On Linux: `cargo run -p mycel-cli -- daemon install`
Expected: writes unit file, enables it.

- [ ] **Step 7: Commit**

```bash
git add crates/mycel-cli/src/supervisor
git commit -m "Add cross-platform daemon supervisor (launchd + systemd)"
```

End of Chunk 7.

---

## Chunk 8: Daemon + end-to-end dogfood

`mycel-daemon` is the long-running watcher. By the end of this chunk, indexing the Mycelium repo on itself works and Tier 1 / `find` queries return sensible results.

### Task 8.1: Daemon with notify watcher + debounce queue

**Files:**
- Modify: `crates/mycel-daemon/src/main.rs`
- Create: `crates/mycel-daemon/src/watcher.rs`
- Create: `crates/mycel-daemon/src/queue.rs`

- [ ] **Step 0: Pin `notify` version exactly + verify imports compile**

In the workspace `Cargo.toml`, change `notify = { version = "8", features = ["macos_fsevent"] }` to a pinned `version = "=8.2"` (or whatever 8.x is current). The notify 8 series has reorganized exports a couple of times; loose minor pinning has bitten downstream crates.

Run a tiny check: add `crates/mycel-daemon/tests/notify_imports.rs`:

```rust
use notify::{recommended_watcher, RecursiveMode, EventKind};

#[test]
fn imports_compile() {
    // Just verifies the items above resolve under the pinned notify
    // version. If this fails, the watcher source needs fixups before
    // proceeding.
    let _ = (RecursiveMode::Recursive, EventKind::Modify(notify::event::ModifyKind::Any));
    fn _coerce<F: Fn(notify::Result<notify::Event>) + Send + 'static>(_f: F) {}
    _coerce(|_| {});
    let _ = recommended_watcher::<fn(notify::Result<notify::Event>)>;
}
```

Run: `cargo test -p mycel-daemon --test notify_imports`
Expected: PASS. If not, consult the notify 8.x changelog for renamed exports.

- [ ] **Step 1: Write `src/watcher.rs`**

```rust
use camino::Utf8PathBuf;
use notify::{Watcher, RecursiveMode, EventKind};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};

pub fn spawn(root: Utf8PathBuf) -> mpsc::Receiver<Utf8PathBuf> {
    let (tx, rx) = mpsc::channel(256);
    tokio::spawn(async move {
        let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
        let mut watcher = match notify::recommended_watcher(move |res| { let _ = raw_tx.send(res); }) {
            Ok(w) => w,
            Err(e) => { warn!(error=%e, "watcher init"); return; }
        };
        if let Err(e) = watcher.watch(root.as_std_path(), RecursiveMode::Recursive) {
            warn!(error=%e, "watch root");
            return;
        }
        info!(root=%root, "watching");

        // simple 2s debounce: collect events into a buffer keyed by path, flush every 2s
        let mut pending: std::collections::HashMap<Utf8PathBuf, ()> = std::collections::HashMap::new();
        let mut tick = tokio::time::interval(Duration::from_secs(2));

        loop {
            tokio::select! {
                Some(ev) = raw_rx.recv() => {
                    if let Ok(ev) = ev {
                        if !matches!(ev.kind, EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)) {
                            continue;
                        }
                        for p in ev.paths {
                            if let Ok(u) = camino::Utf8PathBuf::from_path_buf(p) {
                                if !u.as_str().contains("/.git/") && !u.as_str().contains("/target/") && !u.as_str().contains("/node_modules/") {
                                    pending.insert(u, ());
                                }
                            }
                        }
                    }
                },
                _ = tick.tick() => {
                    for (p, _) in pending.drain() {
                        if tx.send(p).await.is_err() { return; }
                    }
                }
            }
        }
    });
    rx
}
```

- [ ] **Step 2: Write `src/main.rs`**

```rust
mod watcher;

use anyhow::Result;
use camino::Utf8PathBuf;
use mycel_graph::GraphClient;
use mycel_index::Indexer;
use mycel_lsp::MultilspyResolver;
use mycel_models::OllamaEmbedder;
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let log_path = dirs::home_dir().unwrap().join(".cache/mycel/daemon.log");
    if let Some(parent) = log_path.parent() { std::fs::create_dir_all(parent)?; }
    let file_appender = tracing_appender::rolling::daily(log_path.parent().unwrap(), "daemon.log");
    let (nb, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(nb)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    info!("daemon starting");

    // For Phase 1 minimal: read the repo to watch from MYCEL_REPO env (CLI registers this).
    let repo_str = std::env::var("MYCEL_REPO").unwrap_or_else(|_| ".".into());
    let repo: Utf8PathBuf = repo_str.into();
    let repo_canon = repo.canonicalize_utf8()?;

    let url = std::env::var("MYCEL_FALKORDB_URL").unwrap_or_else(|_| "redis://localhost:6379".into());
    let graph_name = format!("mycel:{}", repo_canon.file_name().unwrap_or("repo"));
    let graph = GraphClient::connect(&url, &graph_name).await?;
    let embedder: Arc<dyn mycel_models::Embedder> = Arc::new(OllamaEmbedder::new("http://localhost:11434", "embeddinggemma"));
    let lsp = MultilspyResolver::spawn("python3 scripts/multilspy_bridge.py", repo_canon.clone()).await.ok().map(Arc::new);
    let indexer = Indexer { graph, lsp, embedder };

    // Initial full index.
    info!("starting initial index");
    let n = indexer.index_repo(&repo_canon).await?;
    info!("indexed {n} files");

    // Watch and incrementally re-index.
    let mut rx = watcher::spawn(repo_canon.clone());
    while let Some(path) = rx.recv().await {
        if mycel_extract::for_language(&path).is_none() { continue; }
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                if let Err(e) = indexer.index_file(&path, &content).await {
                    warn!(file=%path, error=%e, "index_file failed");
                }
            }
            Err(_) => { /* file deleted between event and read; skip */ }
        }
    }
    Ok(())
}
```

Add `dirs = "5"` to `mycel-daemon` Cargo.toml deps.

- [ ] **Step 3: Build + smoke**

Run: `cargo build -p mycel-daemon`
Expected: clean.

Run: `MYCEL_REPO=. cargo run -p mycel-daemon` (in another terminal: edit a file in the repo, watch the daemon log)
Expected: initial index completes, then changes trigger re-index within ~2-5s.

- [ ] **Step 4: Commit**

```bash
git add crates/mycel-daemon
git commit -m "Add mycel-daemon with notify watcher and 2s debounce"
```

### Task 8.2: End-to-end dogfood test against Mycelium itself + a TS repo

**Files:**
- Create: `tests/e2e_dogfood.sh`

- [ ] **Step 1: Write the e2e script**

```bash
#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

# This script is NOT part of `cargo test --workspace` because it requires
# FalkorDB + Ollama + multilspy installed on the host. Run manually after
# `just bootstrap`.

# 1. Index the Mycelium repo itself (Rust dogfood).
echo "→ Indexing self..."
cargo run -p mycel-cli --release -- --repo . index .

# 2. Run a few queries.
echo "→ Querying callers of Indexer..."
cargo run -p mycel-cli --release -- --repo . callers Indexer

echo "→ Finding 'index a file with the parser'..."
cargo run -p mycel-cli --release -- --repo . find "index a file with the parser"

# 3. Index the vendored TS fixture (no network dependency).
TS_FIXTURE="tests/fixtures/external/ts-sample"
if [ ! -d "$TS_FIXTURE" ]; then
  echo "TS fixture missing at $TS_FIXTURE — vendor a small TS project there before running e2e" >&2
  echo "(suggested: clone a small TS lib once, drop node_modules + .git, commit the source)" >&2
  exit 1
fi

cargo run -p mycel-cli --release -- --repo "$TS_FIXTURE" index "$TS_FIXTURE"
cargo run -p mycel-cli --release -- --repo "$TS_FIXTURE" find "type-checking utility"

echo "→ E2E PASS"
```

The vendored fixture lives in-repo so the e2e is reproducible and works offline. Pick any small, well-typed TS lib (a few hundred lines, multiple files, no build step needed) and commit just the source files. `tests/fixtures/external/ts-sample/README.md` should record where it was sourced from.

- [ ] **Step 2: Run end-to-end**

Run: `chmod +x tests/e2e_dogfood.sh && tests/e2e_dogfood.sh`
Expected: completes without error; queries return sensible results.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e_dogfood.sh
git commit -m "Add end-to-end dogfood script (self + small TS repo)"
```

End of Chunk 8.

---

## Definition of done (Phase 1)

When all chunks are complete and `tests/e2e_dogfood.sh` passes on both macOS and Linux, Phase 1 is done. The deliverables match the spec's Definition of Done section:

1. ✅ `just bootstrap && mycel index .` works on Mycelium itself (Rust) and a small TS repo.
2. ✅ All Tier 1 queries return correct results.
3. ✅ `mycel find <query>` returns plausible signature-embedded matches.
4. ✅ `mycel daemon install` works on both platforms.
5. ✅ Daemon updates graph within ~5s of a file edit.
6. ✅ All commands support `--json`.
7. ✅ `cargo test --workspace` passes; `cargo clippy --workspace --all-targets -- -D warnings` passes.
8. ✅ README documents bootstrap, tiers, supervisor.

Phase 2 (synthesis) is the next plan — out of scope here.
