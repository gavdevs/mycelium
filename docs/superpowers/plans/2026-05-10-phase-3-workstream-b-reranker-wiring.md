# Phase 3 — Workstream B: Reranker Wiring Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `OllamaReranker::rerank` against the qwen3-reranker family, replace the existing `todo!()`, and wire the config-side `reranker_from_cfg` + `default_reranker_model` helpers so Workstream C can consume a real reranker.

**Architecture:** qwen3-reranker is a yes/no-token scoring model. We call Ollama's `/api/generate` with a `Query: {q}\nDocument: {d}\nRelevant (yes/no):` prompt template, `stream:false`, `temperature:0.0`, parse the response text for `yes`/`no`, return `1.0` / `0.0`. Logits-based scoring is more precise but version-dependent in Ollama; we punt on it and hedge per the design spec ("we'll verify the exact field name against the installed Ollama version during implementation"). Per-candidate calls fan out via `futures::stream::iter().buffer_unordered(N)` with N tied to tier (4 minimal/balanced, 8 max). The whole call respects a 5s timeout via the reqwest client builder. Single-candidate failures get logged at WARN and contribute `f32::NEG_INFINITY` to the result vec; the call returns `Ok(scores)` unless *every* candidate fails (in which case it returns `Err`, and Workstream C's Tier-4 wrapper falls through to cosine ordering).

**Tech Stack:** Rust 2024, tokio, reqwest, futures (workspace dep), async-trait. Tests use the existing `MYCEL_TEST_OLLAMA=1` gate pattern for live-Ollama smoke tests; pure helper functions are tested without HTTP.

**Worktree:** Recommended to run in a dedicated worktree. Suggested name: `phase-3-workstream-b`. Use `superpowers:using-git-worktrees` skill to create.

---

## Chunk 1: Reranker implementation and config plumbing

### Task 1: Add `futures` to `mycel-models` dependencies

**Files:**
- Modify: `crates/mycel-models/Cargo.toml`

- [ ] **Step 1: Add futures to `[dependencies]`**

Edit `crates/mycel-models/Cargo.toml`, add under `[dependencies]` (alphabetical position after `async-trait`):

```toml
futures = { workspace = true }
```

- [ ] **Step 2: Verify the workspace already has futures = "0.3"**

Run: `grep -n "futures" Cargo.toml`
Expected: `futures = "0.3"` present in `[workspace.dependencies]` (already confirmed; sanity check only).

- [ ] **Step 3: Verify the crate still compiles**

Run: `cargo build -p mycel-models`
Expected: clean build, no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/mycel-models/Cargo.toml
git commit -m "Add futures workspace dep to mycel-models for reranker concurrency"
```

---

### Task 2: Extract a pure-function rerank response parser, test it

**Files:**
- Modify: `crates/mycel-models/src/ollama.rs`

Rationale: the response text from `/api/generate` is the only piece we *can* unit test without an HTTP server. Extract the "interpret the model's reply as a score" logic into a pure function and TDD that.

- [ ] **Step 1: Write the failing test**

Append to `crates/mycel-models/src/ollama.rs` (above the file's end, inside a `#[cfg(test)] mod tests { ... }` block — create the block if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_affirmative_responses_as_one() {
        for s in ["yes", "Yes", "  yes  ", "yes.", "yes\n"] {
            assert_eq!(parse_rerank_response(s), 1.0, "expected 1.0 for {s:?}");
        }
    }

    #[test]
    fn parses_negative_responses_as_zero() {
        for s in ["no", "No", "  no  ", "no.", "no\n"] {
            assert_eq!(parse_rerank_response(s), 0.0, "expected 0.0 for {s:?}");
        }
    }

    #[test]
    fn unparseable_responses_are_neg_infinity() {
        for s in ["", "maybe", "definitely", "  "] {
            assert!(parse_rerank_response(s).is_infinite() && parse_rerank_response(s).is_sign_negative(),
                "expected -inf for {s:?}");
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mycel-models -- parse_rerank_response`
Expected: build fails because `parse_rerank_response` doesn't exist.

- [ ] **Step 3: Implement the parser**

Add this free function in `crates/mycel-models/src/ollama.rs` (above `pub struct OllamaReranker`):

```rust
/// Maps a qwen3-reranker-style yes/no completion to a 0/1 score.
///
/// Returns `f32::NEG_INFINITY` for responses we can't classify, so the Tier-4
/// caller can treat them as "no opinion" rather than "definite no". Case- and
/// whitespace-insensitive; only the first lexeme matters because we set
/// `temperature: 0.0` and expect a single-token answer.
fn parse_rerank_response(raw: &str) -> f32 {
    let head = raw.trim().split_whitespace().next().unwrap_or("");
    let head = head.trim_end_matches(|c: char| !c.is_alphanumeric()).to_ascii_lowercase();
    match head.as_str() {
        "yes" => 1.0,
        "no" => 0.0,
        _ => f32::NEG_INFINITY,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p mycel-models -- parse_rerank_response`
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/mycel-models/src/ollama.rs
git commit -m "Add parse_rerank_response helper with yes/no/unknown classification"
```

---

### Task 3: Implement `OllamaReranker::rerank`, replacing the `todo!()`

**Files:**
- Modify: `crates/mycel-models/src/ollama.rs` (the `OllamaReranker` struct and impl)

- [ ] **Step 1: Replace the `OllamaReranker` struct with a fully-built one**

Find this code in `crates/mycel-models/src/ollama.rs`:

```rust
pub struct OllamaReranker {
    pub endpoint: String,
    pub model: String,
}
```

Replace with:

```rust
pub struct OllamaReranker {
    endpoint: String,
    model: String,
    identity: String,
    client: reqwest::Client,
    concurrency: usize,
}

impl OllamaReranker {
    /// `concurrency` is the max in-flight rerank requests; tune to match
    /// Ollama's `num_parallel` (default 4; raise to 8 on Max tier).
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>, concurrency: usize) -> Self {
        let model = model.into();
        let identity = format!("ollama/{model}");
        Self {
            endpoint: endpoint.into(),
            model,
            identity,
            // 5s per call. The Tier-4 pipeline applies its own outer budget;
            // individual stragglers shouldn't drag the whole rerank batch.
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("reqwest client builds with default tls"),
            concurrency: concurrency.max(1),
        }
    }
}
```

- [ ] **Step 2: Replace the `impl Reranker for OllamaReranker` block**

Find the current stub:

```rust
#[async_trait]
impl Reranker for OllamaReranker {
    fn identity(&self) -> &str {
        &self.model
    }
    async fn rerank(&self, _query: &str, _candidates: &[&str]) -> Result<Vec<f32>> {
        todo!("phase 3 — wire reranker endpoint")
    }
}
```

Replace with:

```rust
#[async_trait]
impl Reranker for OllamaReranker {
    fn identity(&self) -> &str {
        &self.identity
    }
    async fn rerank(&self, query: &str, candidates: &[&str]) -> Result<Vec<f32>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        use futures::stream::{self, StreamExt};
        let scored = stream::iter(candidates.iter().enumerate())
            .map(|(i, cand)| {
                let url = format!("{}/api/generate", self.endpoint);
                let client = self.client.clone();
                let model = self.model.clone();
                let identity = self.identity.clone();
                let prompt = build_rerank_prompt(query, cand);
                async move {
                    let body = serde_json::json!({
                        "model": model,
                        "prompt": prompt,
                        "stream": false,
                        "options": {"temperature": 0.0}
                    });
                    let resp_result = client.post(&url).json(&body).send().await;
                    match resp_result.and_then(|r| r.error_for_status()) {
                        Ok(resp) => match resp.json::<GenerateResponse>().await {
                            Ok(parsed) => (i, parse_rerank_response(&parsed.response)),
                            Err(e) => {
                                tracing::warn!(provider = %identity, error = %e, "rerank: bad JSON, scoring as -inf");
                                (i, f32::NEG_INFINITY)
                            }
                        },
                        Err(e) => {
                            tracing::warn!(provider = %identity, error = %e, "rerank: request failed, scoring as -inf");
                            (i, f32::NEG_INFINITY)
                        }
                    }
                }
            })
            .buffer_unordered(self.concurrency)
            .collect::<Vec<_>>()
            .await;
        let mut out = vec![f32::NEG_INFINITY; candidates.len()];
        for (i, score) in scored {
            out[i] = score;
        }
        // If every candidate returned -inf, the reranker is effectively dead;
        // surface that loudly so Tier-4 can fall through to cosine ordering.
        if out.iter().all(|s| s.is_infinite()) {
            return Err(MycelError::Model {
                provider: self.identity.clone(),
                message: "rerank: every candidate failed; reranker is unhealthy".into(),
            });
        }
        Ok(out)
    }
}

fn build_rerank_prompt(query: &str, doc: &str) -> String {
    // Trim docs to ~1500 chars to keep prompt sizes manageable; the reranker
    // doesn't need the whole body, just enough to judge relevance.
    let doc_trunc: String = doc.chars().take(1500).collect();
    format!("Query: {query}\nDocument: {doc_trunc}\nRelevant (yes/no):")
}
```

- [ ] **Step 3: Verify the file compiles**

Run: `cargo build -p mycel-models`
Expected: clean build.

- [ ] **Step 4: Add a unit test for `build_rerank_prompt`**

Append to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn rerank_prompt_includes_query_and_document() {
        let p = build_rerank_prompt("validate auth", "fn check_token() {}");
        assert!(p.contains("Query: validate auth"));
        assert!(p.contains("Document: fn check_token() {}"));
        assert!(p.ends_with("Relevant (yes/no):"));
    }

    #[test]
    fn rerank_prompt_truncates_long_docs() {
        let long_doc = "x".repeat(5000);
        let p = build_rerank_prompt("q", &long_doc);
        // 1500 doc chars + "Query: q\nDocument: " (19 chars) + "\nRelevant (yes/no):" (19 chars) = 1538
        assert!(p.len() <= 1600, "prompt was {} chars", p.len());
    }
```

- [ ] **Step 5: Run unit tests**

Run: `cargo test -p mycel-models`
Expected: all unit tests pass (5 total — 3 parse + 2 prompt).

- [ ] **Step 6: Commit**

```bash
git add crates/mycel-models/src/ollama.rs
git commit -m "Implement OllamaReranker::rerank against qwen3-reranker via /api/generate"
```

---

### Task 4: Add a live-Ollama smoke test for the rerank path

**Files:**
- Create: `crates/mycel-models/tests/rerank.rs`

- [ ] **Step 1: Create the test file**

Write `crates/mycel-models/tests/rerank.rs`:

```rust
use mycel_models::*;

/// Smoke test against a real Ollama with `qwen3-reranker:0.6b` pulled.
///
/// Skipped unless `MYCEL_TEST_OLLAMA=1` to match the existing test posture
/// for live-model tests. Run setup: `ollama pull qwen3-reranker:0.6b`.
#[tokio::test]
async fn ollama_rerank_smoke() {
    if std::env::var("MYCEL_TEST_OLLAMA").ok().as_deref() != Some("1") {
        eprintln!(
            "skipping (set MYCEL_TEST_OLLAMA=1 to run; requires `ollama pull qwen3-reranker:0.6b`)"
        );
        return;
    }
    let r = OllamaReranker::new("http://localhost:11434", "qwen3-reranker:0.6b", 4);
    let scores = r
        .rerank(
            "validate an OAuth bearer token",
            &[
                "fn validate_oauth_bearer(token: &str) -> Result<Claims, AuthError>",
                "fn render_login_button(props: &ButtonProps) -> Element",
            ],
        )
        .await
        .expect("rerank should succeed");
    assert_eq!(scores.len(), 2);
    // The auth-related candidate should score higher than the UI candidate.
    assert!(
        scores[0] >= scores[1],
        "expected auth candidate to outrank UI candidate, got {scores:?}"
    );
}

#[tokio::test]
async fn ollama_rerank_empty_candidates() {
    // Empty input must short-circuit without hitting the network — verify
    // by pointing at an unreachable endpoint.
    let r = OllamaReranker::new("http://127.0.0.1:1", "qwen3-reranker:0.6b", 4);
    let scores = r.rerank("anything", &[]).await.expect("empty must succeed");
    assert!(scores.is_empty());
}
```

- [ ] **Step 2: Run the local-only test (skips live)**

Run: `cargo test -p mycel-models --test rerank`
Expected: 2 tests run; one ("smoke") prints `skipping (...)`, the other ("empty") passes.

- [ ] **Step 3: (Optional) Run live test if Ollama is available**

Manual: pull the model, then `MYCEL_TEST_OLLAMA=1 cargo test -p mycel-models --test rerank -- --nocapture` and verify the smoke test passes.

- [ ] **Step 4: Commit**

```bash
git add crates/mycel-models/tests/rerank.rs
git commit -m "Add live-Ollama smoke test for OllamaReranker"
```

---

### Task 5: Wire `reranker_from_cfg` and `default_reranker_model` in the CLI config layer

**Files:**
- Modify: `crates/mycel-cli/src/config.rs`

- [ ] **Step 1: Add the tier-default helper**

Add this private fn near `default_synthesizer_model` in `crates/mycel-cli/src/config.rs`:

```rust
/// Pick the default reranker model for a tier, mirroring DESIGN.md's
/// tier table. Minimal/Balanced share the small model; Max upgrades to 4b
/// (the swap is free at query time — no reindex implications).
fn default_reranker_model(tier: Option<Tier>) -> String {
    let tier = tier.unwrap_or_else(detect_tier);
    match tier {
        Tier::Minimal | Tier::Balanced => "qwen3-reranker:0.6b".into(),
        Tier::Max => "qwen3-reranker:4b".into(),
    }
}
```

- [ ] **Step 2: Add a unit test for the tier dispatch**

Append (or create) a `#[cfg(test)] mod tests` in `crates/mycel-cli/src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use mycel_core::Tier;

    #[test]
    fn reranker_default_per_tier() {
        assert_eq!(default_reranker_model(Some(Tier::Minimal)), "qwen3-reranker:0.6b");
        assert_eq!(default_reranker_model(Some(Tier::Balanced)), "qwen3-reranker:0.6b");
        assert_eq!(default_reranker_model(Some(Tier::Max)), "qwen3-reranker:4b");
    }
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p mycel-cli --lib reranker_default_per_tier`
Expected: 1 test passes.

- [ ] **Step 4: Add `reranker_from_cfg`**

Add this public fn after `synthesizer_from_cfg` in `crates/mycel-cli/src/config.rs`:

```rust
/// Build a Reranker from config, falling back to the tier-default Ollama
/// model when the user hasn't pinned one. Returns None only if the user
/// explicitly sets `MYCEL_RERANKER=off` (escape hatch for Tier-4 testing
/// without rerank cost — falls through to cosine-only ordering).
pub fn reranker_from_cfg(cfg: &Config) -> Option<std::sync::Arc<dyn mycel_models::Reranker>> {
    if std::env::var("MYCEL_RERANKER").as_deref() == Ok("off") {
        return None;
    }
    let provider = cfg
        .providers
        .reranker
        .clone()
        .unwrap_or_else(ProviderConfig::default_ollama);
    let concurrency = match cfg.models.tier.unwrap_or_else(detect_tier) {
        Tier::Minimal | Tier::Balanced => 4,
        Tier::Max => 8,
    };
    match provider {
        ProviderConfig::Ollama { endpoint, model, .. } => {
            let model = std::env::var("MYCEL_RERANKER_MODEL")
                .ok()
                .or(model)
                .unwrap_or_else(|| default_reranker_model(cfg.models.tier));
            Some(std::sync::Arc::new(mycel_models::OllamaReranker::new(
                endpoint, model, concurrency,
            )))
        }
    }
}
```

- [ ] **Step 5: Verify the crate compiles**

Run: `cargo build -p mycel-cli`
Expected: clean build.

- [ ] **Step 6: Run all cli tests**

Run: `cargo test -p mycel-cli`
Expected: existing tests still pass, plus the new tier-dispatch test.

- [ ] **Step 7: Commit**

```bash
git add crates/mycel-cli/src/config.rs
git commit -m "Wire reranker_from_cfg + default_reranker_model in CLI config layer"
```

---

### Task 6: Full workspace verification

- [ ] **Step 1: Build the workspace**

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 2: Run clippy on the workspace**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: zero warnings (per CLAUDE.md, the clippy gate has caught real bugs and must pass).

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: all tests pass. (Integration tests in `mycel-graph` require an ephemeral FalkorDB on `redis://127.0.0.1:16379`; start with `docker run -d --rm --name mycel-falkordb-test -p 127.0.0.1:16379:6379 falkordb/falkordb:v4.18.3` if not already running.)

- [ ] **Step 4: Confirm `todo!()` is gone from OllamaReranker**

Run: `grep -n "todo!" crates/mycel-models/src/ollama.rs`
Expected: no matches.

- [ ] **Step 5: Final commit if any cleanup**

Only if Steps 1–4 surface follow-up edits. Otherwise nothing left to commit.

---

## Success criteria for this plan

1. `cargo build --workspace` is green.
2. `cargo clippy --workspace --all-targets -- -D warnings` is green.
3. `cargo test --workspace` is green.
4. `OllamaReranker::rerank` is real (no `todo!()`); a live-Ollama smoke test exists and passes when `MYCEL_TEST_OLLAMA=1`.
5. `reranker_from_cfg` returns a usable `Arc<dyn Reranker>` with tier-appropriate defaults; `MYCEL_RERANKER=off` cleanly disables.
6. The PR diff is bounded to: `crates/mycel-models/{Cargo.toml,src/ollama.rs,tests/rerank.rs}` and `crates/mycel-cli/src/config.rs`. Nothing in `mycel-query`, `mycel-index`, `mycel-daemon`, or the multilspy bridge — those are workstreams A / C / D / E.

## Not in this plan (defer to the workstream that owns them)

- Calling `reranker_from_cfg` from any code path — Workstream C wires Tier-4 to consume it.
- Logits-based scoring — explicitly hedged in the design spec; revisit only if eval-harness results in Workstream E show the yes/no parser is the bottleneck.
- Cosine-fallback semantics in `mycel find` — Workstream C concern.
- Sidecar reranker backends (gte / mxbai / jina) — Workstream E plans the eval harness; sidecar support is a separate plan if needed.
