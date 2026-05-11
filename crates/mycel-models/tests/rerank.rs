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
