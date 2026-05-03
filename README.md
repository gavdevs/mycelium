# Mycelium

A local-first, graph-aware code intelligence layer for AI coding agents.

See `DESIGN.md` for the product spec and `docs/superpowers/specs/2026-05-03-phase-1-infrastructure-design.md` for the Phase 1 implementation spec.

## Prerequisites

This project uses [mise](https://mise.jdx.dev) to manage Python, `just`, and other dev-tool versions. Rust is managed by rustup via `rust-toolchain.toml`. Docker and Ollama are system-level prerequisites.

Required:
- [mise](https://mise.jdx.dev/getting-started.html) — runs `mise install` from `mise.toml` (pins Python 3.12, just 1.50.0)
- Rust 1.88+ via [rustup](https://rustup.rs) (auto-installed from `rust-toolchain.toml` on first `cargo` invocation)
- [Docker](https://docs.docker.com/get-docker/) (with the `docker compose` v2 plugin)
- [Ollama](https://ollama.com/download)

Recommended:
- `rust-analyzer` on PATH (faster Rust LSP refinement on first use; Mycelium's `mycel-lsp` falls back to tree-sitter alone if it's missing)
- A TypeScript or Rust project to point Mycelium at (any reasonably-sized repo)

## Quickstart

```sh
mise install     # installs Python + just per mise.toml (one-time on a fresh machine)
just bootstrap   # detects memory tier, starts FalkorDB, pulls Ollama models, installs multilspy, builds
just up          # if you skipped bootstrap and just need FalkorDB running
cargo run -p mycel-cli -- index ./path/to/repo
cargo run -p mycel-cli -- find "the thing that does X"
```

## Hardware tiers

`just bootstrap` auto-detects:
- `minimal` (<12 GB RAM): embeddinggemma + qwen3-reranker:0.6b. No synthesis (Phase 2+).
- `balanced` (12–24 GB RAM): + gemma4:e4b synthesizer.
- `max` (≥24 GB RAM): + qwen3-reranker:4b + qwen3.6:35b-a3b synthesizer.

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
