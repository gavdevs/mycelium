use mycel_models::*;

#[tokio::test]
async fn ollama_embed_smoke() {
    if std::env::var("MYCEL_TEST_OLLAMA").ok().as_deref() != Some("1") {
        eprintln!(
            "skipping (set MYCEL_TEST_OLLAMA=1 to run; requires `ollama pull embeddinggemma`)"
        );
        return;
    }
    let e = OllamaEmbedder::new("http://localhost:11434", "embeddinggemma");
    let out = e.embed(&["the quick brown fox", "lazy dog"]).await.unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].len(), e.dimension() as usize);
}
