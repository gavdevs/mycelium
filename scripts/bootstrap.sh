#!/usr/bin/env bash
set -euo pipefail

# 0. Mise-managed tool versions (python, just) come from mise.toml.
#    Rust is owned by rustup via rust-toolchain.toml, not mise.
#    Verify mise itself is installed; install pinned tools.
if ! command -v mise >/dev/null 2>&1; then
  echo "Missing required tool: mise (https://mise.jdx.dev/getting-started.html)" >&2
  echo "Install mise then re-run this script." >&2
  exit 1
fi

echo "→ Installing mise-pinned tool versions..."
mise install

# Activate mise tools for this script's subprocess scope so cargo/python3/just
# resolve to the mise-managed versions even if the parent shell isn't activated.
eval "$(mise env --shell bash)"

# 1. Verify required system-level tools (not managed by mise)
missing=()
for tool in docker ollama; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing+=("$tool")
  fi
done
# Verify mise-managed tools resolved correctly
for tool in cargo python3 just; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing+=("$tool")
  fi
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "Missing required tools: ${missing[*]}" >&2
  echo "Install them and re-run." >&2
  exit 1
fi

# Verify docker compose plugin (v2) is available
if ! docker compose version >/dev/null 2>&1; then
  echo "Missing required tool: 'docker compose' (v2 plugin)." >&2
  echo "Install Docker Desktop or 'docker-compose-plugin' package." >&2
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
echo "→ Pulling FalkorDB image (no-op if cached)..."
docker compose -f docker/docker-compose.yml pull
echo "→ Starting FalkorDB..."
docker compose -f docker/docker-compose.yml up -d
echo "→ Waiting for FalkorDB to become healthy..."
healthy=0
for _ in $(seq 1 60); do
  if docker exec mycel-falkordb redis-cli ping >/dev/null 2>&1; then
    echo "→ FalkorDB healthy"
    healthy=1
    break
  fi
  sleep 1
done
if [ "$healthy" != "1" ]; then
  echo "FalkorDB did not become healthy after 60s; check 'just logs'" >&2
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

# 5. Install multilspy (into the mise-managed Python's site-packages)
echo "→ Installing multilspy..."
python3 -m pip install --upgrade multilspy

# 6. Build the workspace
echo "→ Building workspace..."
cargo build --workspace

echo
echo "→ Bootstrap complete."
echo "  Try: just up && cargo run -p mycel-cli -- index <path>"
