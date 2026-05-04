use mycel_core::*;
use mycel_graph::GraphClient;
use mycel_models::Embedder;

/// Phase 1 signature-only `mycel find`. Embeds the query string and returns
/// the top-K matching Symbols by cosine similarity, paired with score.
///
/// The graph's IndexManifest (written by `Indexer::index_repo`) is consulted
/// inside `vector_search_top_k` to detect embedder-mismatch — if the indexed
/// vectors used a different model or dimension than the runtime embedder,
/// the search errors loudly rather than silently returning bad results.
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
