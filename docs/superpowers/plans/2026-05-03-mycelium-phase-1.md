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
mycel-core = { path = "../mycel-core" }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
camino = { workspace = true }
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

## Chunk 2 done

End of Chunk 2. The graph layer is now feature-complete for Phase 1 query needs: connection, migrations, symbol/edge upsert, all six Tier 1 queries, manifest read/write, and vector top-K search.

**STOP HERE. Dispatch plan-document-reviewer over Chunks 1-2 before continuing.** If approved, proceed to Chunk 3 (extraction).

The remaining chunks (3 through 8) follow the same structure and will be added in subsequent passes:

- **Chunk 3:** `mycel-extract` — TypeScriptExtractor and RustExtractor with golden snapshot tests.
- **Chunk 4:** `mycel-lsp` — multilspy bridge subprocess and Rust-side `MultilspyResolver`.
- **Chunk 5:** `mycel-models` — `Embedder`/`Synthesizer`/`Reranker` traits + `OllamaEmbedder` impl + Phase-1 stubs.
- **Chunk 6:** `mycel-index` — pipeline orchestration with content-hash dedup.
- **Chunk 7:** `mycel-query` + `mycel-cli` — Tier 1 + signature-only `find` query implementations + the `mycel` binary with clap subcommands and JSON output.
- **Chunk 8:** `mycel-daemon` + cross-platform supervisor + end-to-end dogfood test against the Mycelium repo and a small TS repo.

Each chunk follows the same pattern: failing test → minimal implementation → passing test → commit. Each chunk ends with a checkpoint where a plan-document-reviewer pass should run before proceeding.
