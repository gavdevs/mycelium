#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

# This script is NOT part of `cargo test --workspace` because it requires
# FalkorDB + Ollama + multilspy installed on the host. Run manually after
# `just bootstrap` (or with the dev sidecar as below).

# Allow override of FalkorDB URL for the dev sidecar (default localhost).
: "${MYCEL_FALKORDB_URL:=redis://127.0.0.1:6379}"
export MYCEL_FALKORDB_URL

echo "→ FalkorDB URL: $MYCEL_FALKORDB_URL"

# 1. Index the Mycelium repo itself (Rust dogfood).
echo "→ Indexing self..."
cargo run -p mycel-cli --release -- --repo . index .

# 2. Run a few queries.
echo "→ Querying callers of Indexer..."
cargo run -p mycel-cli --release -- --repo . callers Indexer || true

echo "→ Finding 'index a file with the parser'..."
cargo run -p mycel-cli --release -- --repo . find "index a file with the parser"

# 3. Index the vendored TS fixture (no network dependency).
TS_FIXTURE="tests/fixtures/external/ts-sample"
if [ ! -d "$TS_FIXTURE" ]; then
  echo "TS fixture missing at $TS_FIXTURE — this should have been vendored as part of Phase 1" >&2
  exit 1
fi

echo "→ Indexing $TS_FIXTURE ..."
cargo run -p mycel-cli --release -- --repo "$TS_FIXTURE" index "$TS_FIXTURE"

echo "→ Finding 'type-checking utility'..."
cargo run -p mycel-cli --release -- --repo "$TS_FIXTURE" find "type-checking utility"

echo "→ Querying callers of Cache..."
cargo run -p mycel-cli --release -- --repo "$TS_FIXTURE" callers Cache || true

echo "→ Querying definers 'isExpired'..."
cargo run -p mycel-cli --release -- --repo "$TS_FIXTURE" definers isExpired

echo "→ E2E PASS"
